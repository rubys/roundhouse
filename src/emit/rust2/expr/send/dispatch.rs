//! Receiver-Ty-aware Ruby method bridges. Routes Ruby method calls to
//! Rust analogues based on the receiver's body-typer Ty (`Array`,
//! `Str`, `Sym`, `Hash`, etc.). Mirrors the structure of
//! `typescript/expr.rs`'s recv-ty match block. Returns `Some(emitted)`
//! when a bridge fires; `None` when the recv ty / method combo isn't
//! handled here and should fall through to the generic dispatch path.
//!
//! Also houses [`external_class_method_param_tys`] — the parallel
//! per-method param-Ty table for hand-written runtime modules (`Db`,
//! `Broadcasts`) that aren't surfaced through the
//! `CLASS_METHOD_PARAM_TYS` thread-local.

use crate::expr::{Expr, ExprNode, Literal};

use super::super::util::peel_nil;
use super::super::emit_expr;

/// Per-method positional-param Tys for hand-written runtime modules
/// (`Db`, future `Broadcasts`, etc.) that aren't surfaced through
/// `CLASS_METHOD_PARAM_TYS`. The Const-recv dispatch in emit_send
/// consults this so `Db::prepare(format!(...))` (String arg, `&str`
/// param) inserts the Borrow coercion automatically.
pub(super) fn external_class_method_param_tys(class: &str, method: &str) -> Option<Vec<crate::ty::Ty>> {
    use crate::ty::Ty;
    let hash_str_untyped = || Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(Ty::Untyped),
    };
    match (class, method) {
        ("Db", "prepare") => Some(vec![Ty::Str]),
        ("Db", "exec") => Some(vec![Ty::Str]),
        ("Db", "step") => Some(vec![Ty::Int]),
        ("Db", "column_int") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "column_text") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "column_bool") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "finalize") => Some(vec![Ty::Int]),
        ("Db", "escape_string") => Some(vec![Ty::Str]),
        ("Db", "escape_int") => Some(vec![Ty::Int]),
        ("Db", "escape_bool") => Some(vec![Ty::Bool]),
        ("Db", "last_insert_rowid") => Some(vec![]),
        // `Broadcasts::method(HashMap<String, Value>)` — the lowerer
        // emits kwargs as a HashMap; the runtime shim accepts that
        // shape and pulls named fields out.
        ("Broadcasts", "append" | "prepend" | "replace" | "remove") => {
            Some(vec![hash_str_untyped()])
        }
        _ => None,
    }
}

/// Recv-Ty-aware method bridges — Ruby method calls whose Rust analog
/// differs by receiver type. Predicates retain their trailing `?` at
/// this level. Where Ruby methods return Integer (`.length`, `.size`,
/// `.count`), the bridge inserts `(... as i64)` so downstream
/// arithmetic compiles without per-callsite widens.
pub(super) fn dispatch_method_by_recv_ty(
    recv: &Expr,
    method: &str,
    args: &[Expr],
) -> Option<String> {
    use crate::ty::Ty;
    let recv_s = emit_expr(recv);
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    // Peel `Union<T, Nil>` to `T` for dispatch. The body-typer reports
    // Ruby's nil-on-miss shape but rust2 emits panic-on-miss, so the
    // runtime value really is `T`. Matching `Some(Ty::Str)` directly
    // would miss every `let x = arr[i]` binding (whose recorded Ty is
    // `Union<Str, Nil>`).
    let recv_ty = recv.ty.as_ref().map(peel_nil);
    match recv_ty {
        Some(Ty::Array { .. }) => match method {
            "size" | "length" | "count" if args.is_empty() => {
                Some(format!("({recv_s}.len() as i64)"))
            }
            "empty?" if args.is_empty() => Some(format!("{recv_s}.is_empty()")),
            "any?" if args.is_empty() => Some(format!("!{recv_s}.is_empty()")),
            // `first` / `last` on Vec return Option<&T>; `.cloned()`
            // gives Option<T> matching Ruby's nil-or-value semantics.
            "first" if args.is_empty() => Some(format!("{recv_s}.first().cloned()")),
            "last" if args.is_empty() => Some(format!("{recv_s}.last().cloned()")),
            "to_a" if args.is_empty() => Some(recv_s.clone()),
            // `reverse` / `sort` return new arrays in Ruby. Clone-
            // then-mutate keeps the receiver intact and produces an
            // owned Vec the caller can pass on.
            "reverse" if args.is_empty() => Some(format!(
                "{{ let mut __v = {recv_s}.clone(); __v.reverse(); __v }}"
            )),
            "sort" if args.is_empty() => Some(format!(
                "{{ let mut __v = {recv_s}.clone(); __v.sort(); __v }}"
            )),
            "join" if args.is_empty() => Some(format!("{recv_s}.join(\"\")")),
            "join" if args.len() == 1 => Some(format!("{recv_s}.join({})", args_s[0])),
            // `arr.include?(x)` — Ruby's Array membership test. Rust's
            // `Vec::contains` takes `&T`, so `cols.contains("k")` fails
            // when `cols: Vec<String>`. `iter().any(...)` sidesteps the
            // Borrow constraint.
            "include?" | "contains?" if args.len() == 1 => Some(format!(
                "{recv_s}.iter().any(|__c| __c == {})",
                args_s[0]
            )),
            _ => None,
        },
        Some(Ty::Str) | Some(Ty::Sym) => match method {
            "empty?" if args.is_empty() => Some(format!("{recv_s}.is_empty()")),
            "size" | "length" if args.is_empty() => {
                Some(format!("({recv_s}.len() as i64)"))
            }
            "upcase" if args.is_empty() => Some(format!("{recv_s}.to_uppercase()")),
            "downcase" if args.is_empty() => Some(format!("{recv_s}.to_lowercase()")),
            // `strip` → `trim()` returns &str; `.to_string()` forces
            // owned to match Ruby's `String#strip` return shape.
            "strip" if args.is_empty() => Some(format!("{recv_s}.trim().to_string()")),
            // `reverse` on String — codepoint reversal via chars().
            "reverse" if args.is_empty() => Some(format!(
                "{recv_s}.chars().rev().collect::<String>()"
            )),
            // `chars` returns Array<String> in Ruby; mirror with
            // `Vec<String>` (each char converted to a one-char String).
            "chars" if args.is_empty() => Some(format!(
                "{recv_s}.chars().map(|c| c.to_string()).collect::<Vec<String>>()"
            )),
            "start_with?" if args.len() == 1 => {
                Some(format!("{recv_s}.starts_with({})", args_s[0]))
            }
            "end_with?" if args.len() == 1 => {
                Some(format!("{recv_s}.ends_with({})", args_s[0]))
            }
            "include?" if args.len() == 1 => {
                Some(format!("{recv_s}.contains({})", args_s[0]))
            }
            _ => None,
        },
        Some(Ty::Hash { .. }) => match method {
            // `key?` / `has_key?` / `include?` all probe key presence.
            "key?" | "has_key?" | "include?" if args.len() == 1 => {
                Some(format!("{recv_s}.contains_key({})", args_s[0]))
            }
            "empty?" if args.is_empty() => Some(format!("{recv_s}.is_empty()")),
            "any?" if args.is_empty() => Some(format!("!{recv_s}.is_empty()")),
            "size" | "length" if args.is_empty() => {
                Some(format!("({recv_s}.len() as i64)"))
            }
            "keys" if args.is_empty() => Some(format!(
                "{recv_s}.keys().cloned().collect::<Vec<_>>()"
            )),
            "values" if args.is_empty() => Some(format!(
                "{recv_s}.values().cloned().collect::<Vec<_>>()"
            )),
            "dup" | "clone" if args.is_empty() => Some(format!("{recv_s}.clone()")),
            // `hash.delete(k)` — Ruby removes by key, returns the
            // removed value. The `&` prefix is needed when the arg
            // emits as `String` (owned) but skipped when it's already
            // `&str` (a literal).
            "delete" if args.len() == 1 => {
                let key_emit = if matches!(
                    &*args[0].node,
                    ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
                ) {
                    args_s[0].clone()
                } else {
                    format!("&{}", args_s[0])
                };
                Some(format!("{recv_s}.remove({key_emit})"))
            }
            _ => None,
        },
        _ => None,
    }
}
