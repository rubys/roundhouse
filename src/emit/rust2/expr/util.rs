//! Pure helpers — no thread-local state, no IR walks, just type and
//! string transformations. Extracted from `expr/mod.rs` so the
//! variant-emit files can share them without dragging in the emit
//! match.

use crate::expr::{Expr, ExprNode, Literal};

/// Conservative `Copy`-trait check for Rust target types. Numeric +
/// bool + nil are Copy; String/Vec/HashMap/Option/Class are not.
/// Used by the `Ivar` arm to decide whether a tail-position read
/// needs `.clone()` to avoid moving out of `&self`. `Ty::Untyped`
/// commits to `serde_json::Value` which is non-Copy.
pub(crate) fn is_copy_ty(t: &crate::ty::Ty) -> bool {
    use crate::ty::Ty;
    // Sym maps to `String` in rust2 (see `ty.rs::rust_ty`), so it's
    // non-Copy despite being a primitive-shaped Ruby type.
    matches!(t, Ty::Int | Ty::Bool | Ty::Nil | Ty::Float)
}

/// Peel `Union<T, Nil>` to `T` for dispatch-time matching. Returns the
/// original Ty unchanged if it isn't a 2-variant `T | Nil` union.
/// Mirrors `analyze::body::peel_nilable` — kept locally so emit doesn't
/// reach across into a private analyzer helper.
pub(crate) fn peel_nil(ty: &crate::ty::Ty) -> &crate::ty::Ty {
    use crate::ty::Ty;
    if let Ty::Union { variants } = ty {
        if variants.len() == 2 {
            if let Some(idx) = variants.iter().position(|v| matches!(v, Ty::Nil)) {
                return &variants[1 - idx];
            }
        }
    }
    ty
}

/// True if `ty` is `Option<T>` shape — a `Union { variants }` containing
/// exactly two variants, one of which is `Nil`.
pub(crate) fn is_option_ty(ty: &crate::ty::Ty) -> bool {
    matches!(
        ty,
        crate::ty::Ty::Union { variants }
            if variants.len() == 2 && variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
    )
}

pub(crate) fn is_option_of(outer: &crate::ty::Ty, inner: &crate::ty::Ty) -> bool {
    let crate::ty::Ty::Union { variants } = outer else {
        return false;
    };
    if variants.len() != 2 {
        return false;
    }
    let has_nil = variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil));
    let other = variants
        .iter()
        .find(|v| !matches!(v, crate::ty::Ty::Nil));
    matches!(other, Some(o) if has_nil && o == inner)
}

/// Given a narrowed Ty inside an `is_a?(Class)` branch on a
/// `serde_json::Value`-typed binding, return the `.as_X().unwrap()`
/// (or similar) coercion shape that extracts the inner value, or
/// None if the narrowed Ty doesn't map to a Value accessor.
pub(crate) fn value_narrowing_coercion(narrowed: &crate::ty::Ty) -> Option<&'static str> {
    match narrowed {
        crate::ty::Ty::Str => Some("as_str().unwrap()"),
        crate::ty::Ty::Bool => Some("as_bool().unwrap()"),
        crate::ty::Ty::Int => Some("as_i64().unwrap()"),
        crate::ty::Ty::Float => Some("as_f64().unwrap()"),
        _ => None,
    }
}

/// Synthesize a default-value Rust expression for a missing arg
/// position. Mirrors the Ruby default-arg semantics for the
/// shapes the lowerer-synthesized constructors use (`attrs = {}`
/// → `HashMap::new()`).
/// Render a `dialect::Param.default` Expr as a Rust literal string,
/// when the default is shaped simply enough to round-trip. Handles
/// Ruby's common kwarg-default shapes (`length: 30`,
/// `omission: "..."`, `confirm: true`, …). Returns `None` for
/// non-literal defaults (function calls, conditional exprs); those
/// callers fall back to `synth_default_for_ty`, which only knows the
/// param's Ty and loses the source-level value.
///
/// Closes the gap the `synth_default_for_ty` comment calls out
/// ("Ruby kwargs with non-empty source defaults lose that value at
/// synth time"). The wedge-2c.6 forcing function is
/// `truncate(body, length: 100)` — when only `length` is supplied
/// and `omission` falls back to default, the rendered call needs
/// `"..."` not `""`.
pub(crate) fn render_param_default_literal(expr: &crate::expr::Expr) -> Option<String> {
    use crate::expr::{ExprNode, Literal};
    match &*expr.node {
        ExprNode::Lit { value } => match value {
            Literal::Nil => Some("None".to_string()),
            Literal::Bool { value } => Some(value.to_string()),
            Literal::Int { value } => Some(format!("{value}_i64")),
            Literal::Float { value } => {
                let s = value.to_string();
                Some(if s.contains('.') { s } else { format!("{s}.0") })
            }
            Literal::Str { value } => Some(format!("{value:?}")),
            Literal::Sym { value } => Some(format!("{:?}", value.as_str())),
            _ => None,
        },
        ExprNode::Hash { entries, .. } if entries.is_empty() => {
            Some("std::collections::HashMap::new()".to_string())
        }
        ExprNode::Array { elements, .. } if elements.is_empty() => {
            Some("vec![]".to_string())
        }
        _ => None,
    }
}

pub(crate) fn synth_default_for_ty(ty: &crate::ty::Ty) -> Option<String> {
    use crate::ty::Ty;
    match ty {
        Ty::Hash { .. } => Some("std::collections::HashMap::new()".to_string()),
        Ty::Array { .. } => Some("vec![]".to_string()),
        // `Ty::Str` / `Ty::Sym` at param positions emits as `&str` in
        // rust2 (see `method::rust_param_ty`), so the missing-arg
        // default must be a `&'static str` literal — not the owned
        // `String::new()`. Returning `""` keeps the arg-pad shape
        // type-correct at every method call site. Ruby kwargs with
        // non-empty source defaults (e.g. `omission: "..."`) lose that
        // value at synth time — a separate gap from arity correctness.
        Ty::Str | Ty::Sym => Some("\"\"".to_string()),
        Ty::Int => Some("0_i64".to_string()),
        Ty::Float => Some("0.0_f64".to_string()),
        Ty::Bool => Some("false".to_string()),
        Ty::Untyped => Some("serde_json::Value::Null".to_string()),
        Ty::Nil => Some("None".to_string()),
        // `T | Nil` Union maps to `Option<T>` in the rust2 emitter
        // (see `ty::rust_ty` Union handling). Missing-arg default is
        // `None` — but bare `None` carries no element type, so a
        // downstream caller that disambiguates by signature (e.g.
        // `pub fn x(notice: Option<String>, alert: Option<String>)`
        // called as `x(None, None)` where both Nones leak into a
        // surrounding HashMap literal) hits E0282. Emit the turbofish
        // form so inference always has a concrete element type.
        Ty::Union { variants } => {
            let inner: Vec<&Ty> = variants
                .iter()
                .filter(|v| !matches!(v, Ty::Nil))
                .collect();
            if inner.len() == 1 {
                Some(format!(
                    "Option::<{}>::None",
                    super::super::ty::rust_ty(inner[0])
                ))
            } else {
                Some("None".to_string())
            }
        }
        _ => None,
    }
}

/// True when an arm body's emit is already `Value`-shaped (Ivar
/// read in a class whose field is typed `Untyped`, or a Send
/// already wrapped with `Value::from`). Conservative — over-wraps
/// won't cause a type error since `Value::from(Value)` doesn't
/// impl, so we only skip the wrap on the shapes that emit_expr
/// has already coerced.
pub(crate) fn arm_body_already_value(body: &Expr) -> bool {
    matches!(body.ty.as_ref(), Some(crate::ty::Ty::Untyped))
        || matches!(
            &*body.node,
            ExprNode::Lit { value: Literal::Nil }
        )
}

/// True when `ty` contains a `Ty::Untyped` anywhere — directly or
/// inside a `Union` variant. Used to gate Value-coercion decisions.
pub(crate) fn ty_contains_untyped(ty: &crate::ty::Ty) -> bool {
    use crate::ty::Ty;
    match ty {
        Ty::Untyped => true,
        Ty::Union { variants } => variants.iter().any(ty_contains_untyped),
        _ => false,
    }
}

/// Emit a Case `Pattern` as a Rust `match` arm pattern. The
/// lowerer-synthesized `synth_index_read`/`synth_index_write` use
/// `Pattern::Lit { value: Symbol }` against an `&str`-typed
/// scrutinee — emit as a string-literal pattern. Other shapes fall
/// through to `_` until they're needed.
pub(crate) fn emit_case_pattern(p: &crate::expr::Pattern) -> String {
    use crate::expr::Pattern;
    match p {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Lit { value } => match value {
            Literal::Str { value } => format!("{value:?}"),
            Literal::Sym { value } => format!("{:?}", value.as_str()),
            Literal::Int { value } => value.to_string(),
            Literal::Bool { value } => value.to_string(),
            Literal::Nil => "_".to_string(),
            _ => "_".to_string(),
        },
        Pattern::Bind { name } => name.as_str().to_string(),
        _ => "_".to_string(),
    }
}

/// Indent every non-empty line in `s` by `level` × 4 spaces.
pub(crate) fn indent(s: &str, level: usize) -> String {
    let pad = "    ".repeat(level);
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("{pad}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Built-in container classes whose `[]` / `[]=` should stay as the
/// Rust bracket-index syntax (`HashMap`, `Vec`, etc. via Index/IndexMut).
/// User-defined classes route through the `get_index` / `set_index`
/// method rewrite instead.
pub(crate) fn is_builtin_container_class(name: &str) -> bool {
    let last = name.rsplit("::").next().unwrap_or(name);
    matches!(
        last,
        "Hash" | "HashWithIndifferentAccess" | "Array" | "String"
            | "Flash" | "Session"
            | "Parameters"
            | "Errors" | "ErrorCollection"
    )
}

/// Wrap an emitted RHS with `serde_json::Value::from(...)` when the
/// expression's Ty isn't already `serde_json::Value`. Used by the
/// `set_index` call-site emit (Ty::Class indexer dispatch) — the
/// `def []=(_, untyped)` signature renders the value param as Value,
/// so a String/Int/Bool RHS needs explicit conversion.
pub(crate) fn coerce_to_value(value: &Expr, rhs: &str) -> String {
    use crate::ty::Ty;
    let already_value = matches!(
        value.ty.as_ref(),
        Some(Ty::Untyped)
            | Some(Ty::Var { .. })
            | Some(Ty::Record { .. })
            | Some(Ty::Hash { .. })
    );
    if already_value {
        rhs.to_string()
    } else {
        format!("serde_json::Value::from({rhs})")
    }
}

/// Sanitize a Ruby identifier for Rust:
/// * `foo!` → `foo_bang`. Preserves the distinction vs the non-bang
///   sibling (`def create` vs `def create!` both exist on AR::Base).
/// * `foo?` (predicate) → `foo_pred`. Preserves the distinction vs the
///   same-named reader (every AR column has both a `deleted_at` reader
///   and a `deleted_at?` predicate). Applied unconditionally, mirroring
///   the `!` handling, so the rename reproduces at call sites and
///   hand-written runtime methods conform to the same convention.
/// * `foo=` (setter) → `set_foo`.
/// * `[]` / `[]=` → `get_index` / `set_index`.
/// * Reserved Rust keywords → `r#keyword` raw-identifier form.
pub(crate) fn sanitize_ident(name: &str) -> String {
    if name == "[]" {
        return "get_index".to_string();
    }
    if name == "[]=" {
        return "set_index".to_string();
    }
    // Single-char operator method names ("!", "+", "-", "*", "/", "%",
    // "==", "<=>", etc.). These should never reach `sanitize_ident`
    // — the dedicated `try_*` paths in `ops.rs` handle them — but if
    // one slips through, the `_bang`/`set_` strippers below would
    // produce `_bang` (empty base) or `set_` (empty base) which the
    // call-site then references as a phantom function. Pass through
    // verbatim so the error surfaces at the actual call site instead
    // of as a synthetic identifier collision.
    if name.chars().all(|c| !c.is_alphanumeric() && c != '_') {
        return name.to_string();
    }
    let s = if let Some(base) = name.strip_suffix('!') {
        return format!("{base}_bang");
    } else if let Some(base) = name.strip_suffix('=') {
        return format!("set_{base}");
    } else if let Some(base) = name.strip_suffix('?') {
        return format!("{base}_pred");
    } else {
        name
    };
    if is_rust_keyword(s) {
        format!("r#{s}")
    } else {
        s.to_string()
    }
}

/// Ruby method names → Rust analog. Generic (recv-type-agnostic)
/// table; a richer pass keyed on the receiver's `Ty` can layer on
/// later when ambiguities show up in real emit.
pub(crate) fn rewrite_method_name(m: &str) -> String {
    let bridged = match m {
        "to_s" => "to_string",
        "length" => "len",
        "nil?" => "is_none",
        "empty?" => "is_empty",
        "key?" => "contains_key",
        "has_key?" => "contains_key",
        "include?" => "contains",
        other => other,
    };
    sanitize_ident(bridged)
}

/// Rust 2024 reserved-word set. The `r#ident` raw-identifier form
/// lifts the keyword restriction so user-defined names like `match`,
/// `loop`, `type` can become function/struct names.
pub(crate) fn is_rust_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "break" | "const" | "continue" | "crate" | "else" | "enum"
            | "extern" | "false" | "fn" | "for" | "if" | "impl" | "in"
            | "let" | "loop" | "match" | "mod" | "move" | "mut" | "pub"
            | "ref" | "return" | "self" | "Self" | "static" | "struct"
            | "trait" | "true" | "type" | "unsafe" | "use" | "where"
            | "while" | "async" | "await" | "dyn"
            | "abstract" | "become" | "box" | "do" | "final" | "macro"
            | "override" | "priv" | "typeof" | "unsized" | "virtual"
            | "yield" | "try"
    )
}

#[cfg(test)]
mod predicate_naming_tests {
    use super::sanitize_ident;

    /// Regression guard for the AR column-predicate collision: a column
    /// reader (`deleted_at`) and its predicate (`deleted_at?`) must render
    /// to DISTINCT Rust names. `?`/`!`/`=` get `_pred`/`_bang`/`set_`.
    #[test]
    fn suffix_disambiguation_is_injective() {
        assert_ne!(sanitize_ident("deleted_at"), sanitize_ident("deleted_at?"));
        assert_ne!(sanitize_ident("save"), sanitize_ident("save!"));
        assert_eq!(sanitize_ident("deleted_at?"), "deleted_at_pred");
        assert_eq!(sanitize_ident("save!"), "save_bang");
    }
}
