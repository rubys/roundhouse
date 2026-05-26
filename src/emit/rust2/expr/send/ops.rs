//! Operator-shape and small-pattern recognizers — each `try_*`
//! function probes the `(recv, method, args)` tuple for a specific
//! Ruby idiom and returns `Some(emit)` on a match, `None` otherwise.
//! `emit_send` chains them via `try_*().or_else(...)`-style dispatch.
//!
//! The boundary between "lives here" and "lives in `index.rs`" is
//! size: `[]` / `[]=` accumulated enough sub-cases to warrant its own
//! file; everything else (constructor field assigns, stdlib bridges,
//! binary/unary operators, `<<`) is comparatively compact.

use crate::expr::{Expr, ExprNode};

use super::super::util::peel_nil;
use super::super::{emit_expr, in_constructor, ivar_field_ty};
use super::coerce::coerce_arg_for_field_ty;

/// Constructor `self.field = value` rewrite: inside `pub fn new(...) ->
/// Self` there's no `self` until the closing `Self { ... }` literal,
/// but the lowerer-synthesized `def initialize` body emits `Send {
/// recv: SelfRef, method: "<field>=" }`. Emit as `let <field> = <value>`
/// so the closing struct literal's shorthand binding picks up the local.
pub(super) fn try_constructor_field_assign(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<String> {
    if !in_constructor() || args.len() != 1 || !method.ends_with('=') || method.starts_with('[') {
        return None;
    }
    let r = recv?;
    if !matches!(&*r.node, ExprNode::SelfRef) {
        return None;
    }
    let field = &method[..method.len() - 1];
    // Coerce the value to the struct field's declared type so the
    // closing `Self { ... }` literal's shorthand binding picks up a
    // same-typed local. Field-position coercion differs from
    // param-position: String fields want owned `String`, not the
    // `&str` that `.as_str().unwrap()` produces.
    let rhs = match ivar_field_ty(field) {
        Some(fty) => coerce_arg_for_field_ty(&args[0], &fty),
        None => emit_expr(&args[0]),
    };
    Some(format!("let {field} = {rhs}"))
}

/// Stdlib class-method bridges: `Time.now`, `JSON.generate`,
/// `Base64.encode64`, plus `.utc()` / `.iso8601()` / `.strftime()` on
/// a `Ty::Class { Time }` receiver. The `regex`, `base64`, and
/// `chrono` crates are already rust2 deps.
pub(super) fn try_stdlib_class_method(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<String> {
    let r = recv?;
    if let ExprNode::Const { path } = &*r.node {
        let last = path.last().map(|s| s.as_str()).unwrap_or("");
        match (last, method, args.len()) {
            ("Time", "now", 0) => return Some("chrono::Utc::now()".to_string()),
            ("JSON", "generate" | "dump" | "fast_generate", 1) => {
                return Some(format!("serde_json::to_string(&{}).unwrap()", emit_expr(&args[0])));
            }
            ("JSON", "pretty_generate", 1) => {
                return Some(format!(
                    "serde_json::to_string_pretty(&{}).unwrap()",
                    emit_expr(&args[0])
                ));
            }
            ("Base64", "encode64" | "strict_encode64", 1) => {
                return Some(format!(
                    "{{ use base64::Engine; base64::engine::general_purpose::STANDARD.encode({}) }}",
                    emit_expr(&args[0])
                ));
            }
            ("Base64", "urlsafe_encode64", 1) => {
                return Some(format!(
                    "{{ use base64::Engine; base64::engine::general_purpose::URL_SAFE.encode({}) }}",
                    emit_expr(&args[0])
                ));
            }
            _ => {}
        }
    }
    // `.utc()` on a `Ty::Class { Time }` recv is a no-op (already a
    // chrono DateTime<Utc>). `.iso8601()` → `.to_rfc3339()`;
    // `.strftime(fmt)` → `.format(fmt).to_string()`.
    if matches!(
        r.ty.as_ref().map(peel_nil),
        Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "Time"
    ) {
        match (method, args.len()) {
            ("utc" | "to_time", 0) => return Some(emit_expr(r)),
            ("iso8601" | "rfc3339", 0) => return Some(format!("{}.to_rfc3339()", emit_expr(r))),
            ("rfc2822", 0) => return Some(format!("{}.to_rfc2822()", emit_expr(r))),
            ("strftime", 1) => {
                return Some(format!(
                    "{}.format({}).to_string()",
                    emit_expr(r),
                    emit_expr(&args[0])
                ));
            }
            _ => {}
        }
    }
    None
}

/// Binary operators (==, !=, <, >, +, -, *, /) ingest as Send with
/// `method` as the operator name. Ruby's `+` on strings concatenates;
/// Rust's `&str + &str` doesn't compile (need owned LHS), so emit
/// string concat as `format!("{}{}", a, b)` — handles every
/// (&str/String) combo as single allocations through `format_args!`.
/// Recv-type-aware: only fires on Ty::Str/Ty::Sym receivers; numeric
/// `+` keeps its binary-operator emit below.
pub(super) fn try_binary_operator(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<String> {
    let r = recv?;
    if args.len() != 1 {
        return None;
    }
    if method == "+"
        && matches!(
            r.ty.as_ref(),
            Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym)
        )
    {
        return Some(format!(
            "format!(\"{{}}{{}}\", {}, {})",
            emit_expr(r),
            emit_expr(&args[0]),
        ));
    }
    if matches!(method, "==" | "!=" | "<" | ">" | "<=" | ">=" | "+" | "-" | "*" | "/") {
        // Binary-op LHS is a primary-demanding position. Without
        // the wrap, `x.len() as i64 < y` parses as the start of a
        // turbofish (`i64<y, …>`). Decide pass stamps the bit;
        // `wrap_if_needs_parens` adds the paren only where needed.
        let lhs = super::super::wrap_if_needs_parens(r, emit_expr(r));
        return Some(format!("{} {} {}", lhs, method, emit_expr(&args[0])));
    }
    None
}

/// Unary `!` — `!cond` in Ruby lowers as `Send { recv: cond, method:
/// "!", args: [] }`. Rust uses the same `!` operator syntactically
/// but as a prefix unary, not a method call.
pub(super) fn try_unary_not(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<String> {
    if method != "!" {
        return None;
    }
    // Two surface forms reach here, both meaning "logical not":
    //   Send { recv: Some(x), method: "!", args: [] }   — Ruby's `x.!()`
    //   Send { recv: None,    method: "!", args: [x] }  — view_to_library's
    //                                                     `not_x = send(None, "!", [x])`
    let inner = match (recv, args) {
        (Some(r), []) => r,
        (None, [a]) => a,
        _ => return None,
    };
    Some(format!("!({})", emit_expr(inner)))
}

/// Array append: `arr << x` Ruby idiom → `arr.push(x)` in Rust.
/// Recv-type-aware: only fires for Vec/Array-typed receivers so
/// user-defined `<<` operators on other types stay intact. The arg
/// is coerced to the elem type so `push()` type-checks:
/// `Vec<String>::push` wants owned `String`, but the body-typer
/// often hands us `&str` literals or borrowed `&str`.
pub(super) fn try_array_push(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<String> {
    if method != "<<" || args.len() != 1 {
        return None;
    }
    let r = recv?;
    let Some(crate::ty::Ty::Array { elem }) = r.ty.as_ref() else {
        return None;
    };
    let arg_rendered = match (elem.as_ref(), args[0].ty.as_ref()) {
        (
            crate::ty::Ty::Str | crate::ty::Ty::Sym,
            Some(crate::ty::Ty::Str | crate::ty::Ty::Sym),
        ) => format!("({}).to_string()", emit_expr(&args[0])),
        _ => emit_expr(&args[0]),
    };
    // Use `emit_send_recv` (not `emit_expr`) so the recv-Var clone
    // suppression applies. `Vec::push` is `&mut self`; cloning the
    // recv mutates the discarded copy and the original Vec stays
    // empty (the canonical bug surface was the lowerer-emitted
    // `comments()` body's `results << instance` loop — results
    // stayed empty across iterations and the cascade-delete in
    // `before_destroy` never reached the rows).
    Some(format!("{}.push({})", super::super::emit_send_recv(r), arg_rendered))
}

/// String append: `io << s` Ruby idiom → `io.push_str(&s)` in Rust.
/// Used pervasively in lowered view bodies (`io = String.new; io <<
/// helper(...); io`), where the lowerer tags every site with
/// `IrHint::StringBuilderAppend`. Receiver is unambiguously a local
/// `String` and the arg can be `&str` literal or `String`;
/// `push_str` wants `&str`, so we always borrow.
///
/// Uses `emit_send_recv` (mirroring `try_array_push`) so the
/// SUPPRESS_VAR_CLONE flag suppresses the `.clone()` that the
/// multi-read pre-pass would otherwise append (push_str is
/// `&mut self`; cloning would mutate a discarded copy and lose
/// every append). The pre-pass in `with_method_scope` further
/// skips counting hint-tagged accumulator-recv reads, so for
/// lowerer-synthesized `io` the recv is below the clone threshold
/// anyway — the recv-clone suppression remains as a safety net
/// for user-authored `<<` on Str receivers outside the synthesis.
pub(super) fn try_string_append(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<String> {
    if method != "<<" || args.len() != 1 {
        return None;
    }
    let r = recv?;
    if !matches!(r.ty.as_ref(), Some(crate::ty::Ty::Str | crate::ty::Ty::Sym)) {
        return None;
    }
    let arg = &args[0];
    let arg_rendered = match arg.ty.as_ref() {
        Some(crate::ty::Ty::Str | crate::ty::Ty::Sym) => match &*arg.node {
            ExprNode::Lit { value: crate::expr::Literal::Str { .. } } => emit_expr(arg),
            _ => format!("&{}", emit_expr(arg)),
        },
        _ => format!("&{}.to_string()", emit_expr(arg)),
    };
    Some(format!("{}.push_str({arg_rendered})", super::super::emit_send_recv(r)))
}
