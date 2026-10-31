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

use super::super::emit_expr;
use super::super::ivar_field_ty;
use super::super::util::{is_option_ty, peel_nil};

fn expr_is_option(expr: &Expr) -> bool {
    column_ty(expr).as_ref().is_some_and(is_option_ty)
}

fn option_column_is_text(expr: &Expr) -> bool {
    column_ty(expr)
        .as_ref()
        .is_some_and(|ty| matches!(peel_nil(ty), crate::ty::Ty::Str | crate::ty::Ty::Sym))
}

fn column_ty(expr: &Expr) -> Option<crate::ty::Ty> {
    let ExprNode::Ivar { name } = &*expr.node else {
        return None;
    };
    ivar_field_ty(name.as_str())
}

/// Per-method positional-param Tys for hand-written runtime modules
/// (`Db`, future `Broadcasts`, etc.) that aren't surfaced through
/// `CLASS_METHOD_PARAM_TYS`. The Const-recv dispatch in emit_send
/// consults this so `Db::prepare(format!(...))` (String arg, `&str`
/// param) inserts the Borrow coercion automatically.
pub(super) fn external_class_method_param_tys(
    class: &str,
    method: &str,
) -> Option<Vec<crate::ty::Ty>> {
    use crate::ty::Ty;
    let hash_str_untyped = || Ty::Hash {
        key: std::sync::Arc::new(Ty::Str),
        value: std::sync::Arc::new(Ty::Untyped),
    };
    let str_opt = || Ty::Union {
        variants: vec![Ty::Str, Ty::Nil].into(),
    };
    match (class, method) {
        ("Db", "prepare") => Some(vec![Ty::Str]),
        ("Db", "exec") => Some(vec![Ty::Str]),
        ("Db", "step") => Some(vec![Ty::Int]),
        ("Db", "column_int") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "column_float") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "column_text") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "column_bool") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "finalize") => Some(vec![Ty::Int]),
        ("Db", "escape_string") => Some(vec![Ty::Str]),
        ("Db", "escape_int") => Some(vec![Ty::Int]),
        ("Db", "escape_bool") => Some(vec![Ty::Bool]),
        // Nullable-column seam: nil renders the SQL keyword NULL. The
        // param types drive the arg coercion (an owned `Option<String>`
        // field borrows to the `Option<&str>` param via `.as_deref()`).
        ("Db", "column_int_opt") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "column_float_opt") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "column_text_opt") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "column_bool_opt") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "escape_string_opt") => {
            Some(vec![Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() }])
        }
        ("Db", "escape_int_opt") => Some(vec![Ty::Union { variants: vec![Ty::Int, Ty::Nil].into() }]),
        ("Db", "escape_float_opt") => {
            Some(vec![Ty::Union { variants: vec![Ty::Float, Ty::Nil].into() }])
        }
        ("Db", "escape_bool_opt") => Some(vec![Ty::Union { variants: vec![Ty::Bool, Ty::Nil].into() }]),
        ("Db", "last_insert_rowid") => Some(vec![]),
        ("Db", "changes") => Some(vec![]),
        // `Broadcasts::method(HashMap<String, Value>)` — the lowerer
        // emits kwargs as a HashMap; the runtime shim accepts that
        // shape and pulls named fields out.
        ("Broadcasts", "append" | "prepend" | "replace" | "remove") => {
            Some(vec![hash_str_untyped()])
        }
        // The broadcast-render bracket pair the broadcast lowerings
        // wrap synthesized payloads in. Both params render as `&str`
        // on the rust runtime, so the owned-String args (a
        // `begin_broadcast_render()` call and a `Views::…` render)
        // each need the `&(...)` borrow the coercer inserts from
        // these declared types.
        ("ViewHelpers", "broadcast_render") => Some(vec![Ty::Str, Ty::Str]),
        ("ViewHelpers", "begin_broadcast_render") => Some(vec![]),
        // Runtime files emit under an empty EmitCtx (see rust.rs
        // rust_units), so Const-recv ActionController helpers are
        // invisible to the global registry. Pin RBS param Tys here
        // so Family 4/6 fire for HeaderStore/Base call sites.
        ("ActionController", "header_key_ok?") => Some(vec![str_opt()]),
        ("ActionController", "header_value_ok?") => Some(vec![str_opt()]),
        ("ActionController", "header_control?") => Some(vec![Ty::Str]),
        ("ActionController", "sanitize_location") => Some(vec![Ty::Str]),
        ("ActionController", "location_host") => Some(vec![Ty::Str]),
        ("ActionController", "find_substr") => Some(vec![Ty::Str, Ty::Str]),
        ("ActionController", "find_last") => Some(vec![Ty::Str, Ty::Str]),
        ("ActionController", "csrf_token_valid?") => Some(vec![Ty::Str, Ty::Str]),
        _ => None,
    }
}

/// Per-method param-Ty fallback for the per-controller AC::Base shim
/// methods (`render`, `render_with`, `redirect_to`, `head`,
/// `request_format`). These are hand-coded strings appended to each
/// controller's emitted file by `rust.rs::emit` (see the `ac_shim`
/// format-string around line 612) — they aren't LCs, so they don't
/// surface through `collect_class_method_param_tys` nor through the
/// `runtime_lcs` forwarded into the global registry.
///
/// Returns `Some(Ty)` only for arg positions that need coercion (the
/// trailing opts-Hash arg for `render_with`/`redirect_to`). The
/// owned-String body / url arg on `render`/`render_with`/
/// `redirect_to` returns `None` so the caller emits bare (no Family
/// 4 borrow wrap — those shim signatures take `String` not `&str`).
///
/// Keep these entries in lockstep with the literal shim text in
/// `rust.rs::emit`. If/when the shim's signature changes (e.g. to
/// match the AC::Base .rbs contract), update both.
pub(super) fn controller_shim_method_param_ty(method: &str, idx: usize) -> Option<crate::ty::Ty> {
    use crate::ty::Ty;
    let hash_str_untyped = || Ty::Hash {
        key: std::sync::Arc::new(Ty::Str),
        value: std::sync::Arc::new(Ty::Untyped),
    };
    match (method, idx) {
        ("render_with", 1) => Some(hash_str_untyped()),
        ("redirect_to", 1) => Some(hash_str_untyped()),
        ("head", 1) => Some(hash_str_untyped()),
        _ => None,
    }
}

/// Total positional arity of an AC::Base shim method. Used by the
/// SelfRef-recv dispatch in `emit_send` to pad missing trailing args
/// with synth defaults — Ruby `head :sym` (no kwargs) calls the same
/// underlying shape as `head :sym, content_type: ...`, but Rust needs
/// the explicit `HashMap::new()` to satisfy the shim's fixed arity.
/// Keep in lockstep with the shim text in `rust.rs::emit` and the
/// per-index `controller_shim_method_param_ty` table above.
pub(super) fn controller_shim_arity(method: &str) -> Option<usize> {
    match method {
        "render" => Some(1),
        "render_with" => Some(2),
        "request_format" => Some(0),
        "redirect_to" => Some(2),
        "head" => Some(2),
        _ => None,
    }
}

/// `to_s` on a Value-shaped recv. Hash/Session `#get` rust-emits
/// `Option<T>` even when IR types the result as Untyped/Union — use
/// `unwrap_or_default` (Ruby `nil.to_s` is `""`). Union params and
/// Hash `[]`/`fetch` that rust-emit as `serde_json::Value` use UFCS
/// so files without `use RubyToS` compile. Untyped/Record and other
/// Sends keep method-style `.ruby_to_s()`.
fn ruby_to_s_emit(recv: &Expr, recv_s: &str, ufcs: bool) -> String {
    use super::super::util::is_option_ty;
    use crate::ty::Ty;
    match &*recv.node {
        ExprNode::Send { method: m, .. } if m.as_str() == "get" => {
            format!("{recv_s}.clone().unwrap_or_default()")
        }
        ExprNode::Send {
            method: m,
            recv: inner,
            ..
        } if m.as_str() == "[]" => {
            let ivar_ty = match inner.as_ref().map(|e| &*e.node) {
                Some(ExprNode::Ivar { name }) => super::super::ivar_field_ty(name.as_str()),
                _ => None,
            };
            // Prefer the struct-field table: HeaderStore `@vals` is
            // Array[Option<String>] there, while the body-typer still
            // reports Array[Untyped] from `@vals = []`.
            let recv_ty = ivar_ty
                .as_ref()
                .or_else(|| inner.as_ref().and_then(|e| e.ty.as_ref()))
                .map(peel_nil);
            match recv_ty {
                Some(Ty::Class { id, .. })
                    if {
                        let s = id.0.as_str();
                        let leaf = s.rsplit("::").next().unwrap_or(s);
                        matches!(leaf, "Session" | "Flash")
                    } =>
                {
                    format!("{recv_s}.clone().unwrap_or_default()")
                }
                Some(Ty::Array { elem }) if is_option_ty(elem) => {
                    format!("{recv_s}.clone().unwrap_or_default()")
                }
                Some(Ty::Array { .. }) => format!("{recv_s}.to_string()"),
                // An element of a Value-valued Hash is a Value whatever
                // the read's own type says: qualified, so a model file
                // that does not `use RubyToS` compiles.
                Some(Ty::Hash { value, .. }) if super::super::super::ty::rust_value_shaped(value) => {
                    format!("<serde_json::Value as crate::http::RubyToS>::ruby_to_s(&({recv_s}))")
                }
                Some(ty)
                    if ufcs
                        && (super::super::super::ty::rust_value_shaped(ty)
                            || matches!(ty, Ty::Hash { .. })) =>
                {
                    format!("<serde_json::Value as crate::http::RubyToS>::ruby_to_s(&({recv_s}))")
                }
                _ if ufcs => {
                    format!("<serde_json::Value as crate::http::RubyToS>::ruby_to_s(&({recv_s}))")
                }
                _ => format!("{recv_s}.ruby_to_s()"),
            }
        }
        ExprNode::Var { .. } | ExprNode::Ivar { .. } if ufcs => {
            format!("<serde_json::Value as crate::http::RubyToS>::ruby_to_s(&({recv_s}))")
        }
        ExprNode::Send { method: m, .. } if ufcs && matches!(m.as_str(), "fetch") => {
            format!("<serde_json::Value as crate::http::RubyToS>::ruby_to_s(&({recv_s}))")
        }
        _ => format!("{recv_s}.ruby_to_s()"),
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
    // Use `emit_send_recv` so the `NEEDS_PARENS` decide-pass bit
    // wraps non-primary recvs (`x.len() as i64`, casts, etc.)
    // before the per-method bridge appends `.<rust_method>(...)`.
    // Mirrors the bare-var clone-suppression path too.
    let raw_recv_s = super::super::emit_send_recv(recv);
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    // Peel `Union<T, Nil>` to `T` for dispatch. The body-typer reports
    // Ruby's nil-on-miss shape but rust emits panic-on-miss, so the
    // runtime value really is `T`. Matching `Some(Ty::Str)` directly
    // would miss every `let x = arr[i]` binding (whose recorded Ty is
    // `Union<Str, Nil>`).
    let recv_ty = recv.ty.as_ref().map(peel_nil);
    // The Rust binding for a Var recv may be `Option<T>` even when
    // the body-typer already narrowed e.ty to `T` (after `if !x
    // .nil?`). Without unwrap, `notice.is_empty()` on
    // `notice: Option<String>` raises E0599. Gate on the PARAM_TYPES/
    // LOCAL_VAR_TYPES side so only true Option-typed locals get the
    // unwrap — `let pp = vec[i].clone()` records `Ty::Str` in
    // LOCAL_VAR_TYPES (the assignment unwraps the body-typer's
    // `Option<String>` Hash/Array index Ty), so dispatch on pp
    // skips the wrap.
    // The view lowerer emits a bare param read as `Send { recv: None,
    // method: X, args: [] }` (Ruby implicit-self). At emit time the
    // param-shadow check in `emit_send` rewrites it back to a Var
    // read, but as a *recv* the shape is still the Send node. Treat
    // both shapes as Var-like for the binding-is-Option check.
    let var_name: Option<&str> = match &*recv.node {
        ExprNode::Var { name, .. } => Some(name.as_str()),
        ExprNode::Send {
            recv: None,
            method,
            args,
            ..
        } if args.is_empty() => {
            let n = method.as_str();
            if super::super::param_ty(n).is_some() {
                Some(n)
            } else {
                None
            }
        }
        _ => None,
    };
    // Only insert `.clone().unwrap()` here when the Var emit DIDN'T
    // already do so. Var emit's narrowing-write-back at expr/mod.rs
    // fires when `e.ty` is the narrowed type (T) but the declared
    // binding is `Option<T>` — and produces `name.clone().unwrap()`.
    // If recv.ty matches the declared binding (still Option), Var
    // emit didn't unwrap and we need to. If recv.ty equals the
    // peeled type, Var emit already unwrapped and we'd be doubling.
    // Only check PARAM_TYPES (not LOCAL_VAR_TYPES) here: function
    // params are declared with their exact Rust signature, so
    // `param_ty("notice") = Option<String>` faithfully reflects the
    // runtime binding. LOCAL_VAR_TYPES, by contrast, stores the
    // body-typer view at assign time — which says `Option<T>` for a
    // `let x = arr[i].clone()` even though rust's emit unwraps at
    // assign time to make x a plain `String`. Including locals here
    // would double-unwrap (`pp.clone().clone().unwrap()` on a
    // String-typed pp in router.rs).
    let binding_is_option = match var_name {
        Some(n) => {
            let declared = super::super::param_ty(n);
            let is_opt = matches!(declared, Some(ref t) if super::super::util::is_option_ty(t));
            let still_option_at_recv = matches!(
                recv.ty.as_ref(),
                Some(t) if super::super::util::is_option_ty(t)
            );
            is_opt && still_option_at_recv
        }
        None => false,
    };
    // Not the narrowed read's type: a param the signature renders as a Value stays a Value where the read emits as the bare name
    let bound_value_ty = var_name
        .filter(|n| raw_recv_s.trim_end_matches(".clone()") == *n)
        .and_then(super::super::param_ty)
        .filter(|t| super::super::super::ty::rust_ty(t) == "serde_json::Value")
        .filter(|_| recv_ty.is_some_and(|t| super::super::super::ty::rust_ty(t) != "serde_json::Value"));
    let recv_ty = if bound_value_ty.is_some() { bound_value_ty.as_ref() } else { recv_ty };
    let recv_s = if binding_is_option {
        // `.clone().unwrap()` is a method chain — already a primary
        // form. The outer wrap was historically defensive against
        // downstream `.method` chaining; the `NEEDS_PARENS` decide-
        // pass now drives wrap insertion at the consumer side instead.
        format!("{raw_recv_s}.clone().unwrap()")
    } else {
        raw_recv_s
    };
    match recv_ty {
        Some(Ty::Array { elem }) => match method {
            // An element that is never nil or false makes `none?` mean no elements at all
            "none?" if args.is_empty() && matches!(**elem, Ty::Str | Ty::Int | Ty::Float | Ty::Sym) => {
                Some(format!("{recv_s}.is_empty()"))
            }
            "size" | "length" | "count" if args.is_empty() => {
                // The `as i64` cast makes this non-primary. The
                // decide pass stamps `NEEDS_PARENS` on this Send
                // when used as a chained recv; consumer wraps.
                Some(format!("{recv_s}.len() as i64"))
            }
            "empty?" if args.is_empty() => Some(format!("{recv_s}.is_empty()")),
            "any?" if args.is_empty() => Some(format!("!{recv_s}.is_empty()")),
            // `first` / `last` on Vec return Option<&T>; `.cloned()`
            // gives Option<T> matching Ruby's nil-or-value semantics.
            "first" if args.is_empty() => Some(format!("{recv_s}.first().cloned()")),
            "last" if args.is_empty() => Some(format!("{recv_s}.last().cloned()")),
            "to_a" if args.is_empty() => Some(recv_s.clone()),
            // `ary.index(x)`: the first position holding x, or nil
            "index" if args.len() == 1 => Some(format!(
                "{recv_s}.iter().position(|__x| *__x == {}).map(|__i| __i as i64)",
                args_s[0]
            )),
            // `reverse` / `sort` return new arrays in Ruby. Clone-
            // then-mutate keeps the receiver intact and produces an
            // owned Vec the caller can pass on.
            "reverse" if args.is_empty() => Some(format!(
                "{{ let mut __v = {recv_s}.clone(); __v.reverse(); __v }}"
            )),
            "sort" if args.is_empty() => Some(format!(
                "{{ let mut __v = {recv_s}.clone(); __v.sort(); __v }}"
            )),
            // Not `Vec::join` alone: it joins only strings, where Ruby's `join` writes each element's `to_s`, nil as "".
            "join" if args.len() <= 1 && !matches!(**elem, Ty::Str) => {
                let sep = args_s.first().cloned().unwrap_or_else(|| "\"\"".to_string());
                let to_s = match &**elem {
                    Ty::Int | Ty::Bool => "v.to_string()",
                    Ty::Nil => "String::new()",
                    // Only a `map` result: a runtime Array declared `Array[String?]` is a `Vec<String>` in Rust.
                    Ty::Union { variants }
                        if matches!(&*recv.node, ExprNode::Send { method, .. } if matches!(method.as_str(), "map" | "collect"))
                            && variants.len() == 2
                            && variants.iter().any(|v| matches!(v, Ty::Nil))
                            && variants.iter().any(|v| matches!(v, Ty::Str)) =>
                    {
                        "v.clone().unwrap_or_default()"
                    }
                    // Not Float: `f64`'s `to_string` writes 1.0 as "1", so it stays a build error rather than a wrong string.
                    _ => return Some(format!("{recv_s}.join({sep})")),
                };
                Some(format!("{recv_s}.iter().map(|v| {to_s}).collect::<Vec<String>>().join(&{sep})"))
            }
            "join" if args.is_empty() => Some(format!("{recv_s}.join(\"\")")),
            "join" if args.len() == 1 => Some(format!("{recv_s}.join({})", args_s[0])),
            // `arr.include?(x)` — Ruby's Array membership test. Rust's
            // `Vec::contains` takes `&T`, so `cols.contains("k")` fails
            // when `cols: Vec<String>`. `iter().any(...)` sidesteps the
            // Borrow constraint. Dereference the iter item so both
            // bool == bool and String == &str comparisons work; `==`
            // borrows its operands, so non-Copy elements are not moved.
            "include?" | "contains?" if args.len() == 1 => {
                // A column is `Option<T>`. Compare the `Some` so nil is
                // simply absent from the list.
                if expr_is_option(&args[0]) {
                    // String columns are `Option<String>`; list elements
                    // are `&str`. `as_deref` compares `&str`. Numbers
                    // stay `Option<&T> == Some(&T)`.
                    if option_column_is_text(&args[0]) {
                        Some(format!(
                            "{recv_s}.iter().any(|__c| {}.as_deref() == Some(*__c))",
                            args_s[0]
                        ))
                    } else {
                        Some(format!(
                            "{recv_s}.iter().any(|__c| {}.as_ref() == Some(__c))",
                            args_s[0]
                        ))
                    }
                } else {
                    Some(format!("{recv_s}.iter().any(|__c| *__c == {})", args_s[0]))
                }
            }
            _ => None,
        },
        Some(Ty::Str) | Some(Ty::Sym) => {
            // String kernel, also the Symbol arm this match has always
            // shared. Spellings live in `catalog::string_kernel`.
            if let Some(entry) = crate::catalog::string_kernel::lookup(method, args.len()) {
                return Some(crate::catalog::string_kernel::render_rust(
                    entry, &recv_s, &args_s,
                ));
            }
            match method {
                "to_f" if args.is_empty() => {
                    Some(format!("{recv_s}.parse::<f64>().unwrap_or(0.0)"))
                }
                // `reverse` on String — codepoint reversal via chars().
                "reverse" if args.is_empty() => {
                    Some(format!("{recv_s}.chars().rev().collect::<String>()"))
                }
                // `chars` returns Array<String> in Ruby; mirror with
                // `Vec<String>` (each char converted to a one-char String).
                "chars" if args.is_empty() => Some(format!(
                    "{recv_s}.chars().map(|c| c.to_string()).collect::<Vec<String>>()"
                )),
                // `String#match?(re)` → `re.is_match(str)`. Rust has no
                // `str::match_pred`; the Regex is the natural receiver
                // (mirrors the TypeScript `re.test(s)` flip).
                "match?" if args.len() == 1 => {
                    Some(format!("{}.is_match(&*({recv_s}))", args_s[0]))
                }
                _ => None,
            }
        }
        // `Regexp#match?(str)` → `re.is_match(str)` when the receiver
        // is already the pattern (validations, flipped call sites).
        Some(Ty::Class { id, .. }) if id.0.as_str() == "Regexp" => match method {
            "match?" if args.len() == 1 => {
                // `Regexp#match?(nil)` is false. Unwrapping an absent
                // column panics.
                if expr_is_option(&args[0]) {
                    Some(format!(
                        "{}.as_deref().is_some_and(|__s| {recv_s}.is_match(__s))",
                        args_s[0]
                    ))
                } else {
                    Some(format!("{recv_s}.is_match(&*({}))", args_s[0]))
                }
            }
            _ => None,
        },
        // `Untyped` recv (rust's alias for `serde_json::Value`) +
        // `Record` (a sub-Hash through `.as_object().iter()`).
        // `to_s` on these is the Ruby `Object#to_s` shape — for
        // String variants the bare inner string, for everything
        // else JSON-encode. Rust's `serde_json::Value::to_string()`
        // unconditionally JSON-encodes, which breaks attribute
        // emission (`data-turbo-track="reload"` becomes
        // `data-turbo-track="\"reload\""`, and form `value=` quotes
        // a title). Route through the `RubyToS` trait (defined in
        // `runtime/rust/http.rs`): compile-time dispatch picks the
        // right impl for `str` / `String` / `serde_json::Value`.
        // Files that import the trait (json_builder, view_helpers)
        // can use method-style `.ruby_to_s()`.
        // `Integer#to_i` is the identity. Its common receiver is an
        // index read (`arr[i].to_i`), which types `Integer | nil` but
        // renders as the `i64` itself (see the nil peel above).
        Some(Ty::Int) => match method {
            "to_i" if args.is_empty() => Some(recv_s.to_string()),
            _ => None,
        },
        Some(Ty::Untyped) | Some(Ty::Record { .. }) => match method {
            "to_s" if args.is_empty() => Some(ruby_to_s_emit(recv, &recv_s, false)),
            _ => None,
        },
        // Heterogeneous unions / ParamValue / Var that rust-emit as
        // Value (e.g. `optional_value_attr`'s column union). Fully-
        // qualified so files that don't `use RubyToS` still compile
        // — a blanket `.ruby_to_s()` here broke ActionController on
        // compare rust.
        Some(ty) if super::super::super::ty::rust_value_shaped(ty) => match method {
            "to_s" if args.is_empty() => Some(ruby_to_s_emit(recv, &recv_s, true)),
            // Ruby's to_f on an Integer, a Float, or a String's leading number; nil and the rest are 0.0
            "to_f" if args.is_empty() => Some(format!(
                "{{ let __v = {recv_s}; match &__v {{ serde_json::Value::String(__s) => __s.trim().parse::<f64>().unwrap_or(0.0), _ => __v.as_f64().unwrap_or(0.0) }} }}"
            )),
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
                // `as i64` cast — non-primary. Decide pass stamps
                // `NEEDS_PARENS` on this Send for chained-recv use;
                // consumer wraps. Arg/let-RHS positions stay bare.
                Some(format!("{recv_s}.len() as i64"))
            }
            "keys" if args.is_empty() => {
                Some(format!("{recv_s}.keys().cloned().collect::<Vec<_>>()"))
            }
            "values" if args.is_empty() => {
                Some(format!("{recv_s}.values().cloned().collect::<Vec<_>>()"))
            }
            "dup" | "clone" if args.is_empty() => Some(format!("{recv_s}.clone()")),
            // `hash.delete(k)` — Ruby removes by key, returns the
            // removed value. The `&` prefix is needed when the arg
            // emits as `String` (owned) but skipped when it's already
            // `&str` (a literal).
            "delete" if args.len() == 1 => {
                let key_emit = if matches!(
                    &*args[0].node,
                    ExprNode::Lit {
                        value: Literal::Str { .. } | Literal::Sym { .. }
                    }
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

#[cfg(test)]
mod tests {
    use super::super::super::emit_expr;
    use super::super::super::with_emit_ctx;
    use crate::catalog::string_kernel::{STRING_KERNEL, StringKernel};
    use crate::diagnostic::Severity;
    use crate::emit::crystal;
    use crate::emit::rust::EmitCtx;
    use crate::emit::typescript;
    use crate::expr::{Expr, ExprNode, Literal};
    use crate::ident::Symbol;
    use crate::span::Span;
    use crate::ty::Ty;
    use std::path::PathBuf;

    fn rust_emit(expr: &Expr) -> String {
        with_emit_ctx(EmitCtx::default(), || emit_expr(expr))
    }

    /// Oracle for one call on the literal `"ab"` (and `"a"` when the
    /// row takes an argument). These are the spellings the bridges
    /// emitted before the table existed.
    fn emitted(name: &str) -> (&'static str, &'static str, &'static str) {
        match name {
            "empty?" => (
                r#""ab".is_empty()"#,
                r#""ab".length === 0"#,
                r#""ab".empty?"#,
            ),
            "length" | "size" => (
                r#""ab".chars().count() as i64"#,
                r#""ab".length"#,
                r#""ab".size.to_i64"#,
            ),
            "upcase" => (
                r#""ab".to_uppercase()"#,
                r#""ab".toUpperCase()"#,
                r#""ab".upcase"#,
            ),
            "downcase" => (
                r#""ab".to_lowercase()"#,
                r#""ab".toLowerCase()"#,
                r#""ab".downcase"#,
            ),
            "strip" => (
                r#""ab".trim().to_string()"#,
                r#""ab".trim()"#,
                r#""ab".strip"#,
            ),
            "start_with?" => (
                r#""ab".starts_with(&*("a"))"#,
                r#""ab".startsWith("a")"#,
                r#""ab".starts_with?("a")"#,
            ),
            "end_with?" => (
                r#""ab".ends_with(&*("a"))"#,
                r#""ab".endsWith("a")"#,
                r#""ab".ends_with?("a")"#,
            ),
            "include?" => (
                r#""ab".contains(&*("a"))"#,
                r#""ab".includes("a")"#,
                r#""ab".includes?("a")"#,
            ),
            "to_i" => (
                r#""ab".parse::<i64>().unwrap_or(0)"#,
                r#"Number("ab")"#,
                r#""ab".to_i64"#,
            ),
            other => panic!("no emit oracle for `{other}`"),
        }
    }

    fn call_source(entry: &StringKernel) -> String {
        if entry.arity == 0 {
            format!("\"ab\".{}", entry.name)
        } else {
            format!("\"ab\".{}(\"a\")", entry.name)
        }
    }

    fn find_send(expr: &Expr, method: &str) -> Expr {
        let mut found = None;
        let mut pending = vec![expr];
        while let Some(current) = pending.pop() {
            if let ExprNode::Send { method: name, .. } = &*current.node {
                if name.as_str() == method {
                    found = Some(current.clone());
                }
            }
            current
                .node
                .for_each_child(&mut |child| pending.push(child));
        }
        found.unwrap_or_else(|| panic!("no `{method}` send"))
    }

    fn analyze_method(source: &str, method_name: &str) -> Expr {
        let mut app = crate::ingest::ingest_app_from_tree(crate::vfs::FileTree::from([(
            PathBuf::from("app/lib/probe.rb"),
            source.as_bytes().to_vec(),
        )]))
        .unwrap();
        crate::analyze::Analyzer::new(&app).analyze(&mut app);
        let errors: Vec<_> = crate::analyze::diagnose(&app)
            .into_iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| d.message)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
        let class = app
            .library_classes
            .iter()
            .find(|class| class.name.0.as_str() == "Probe")
            .expect("Probe");
        let method = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == method_name)
            .unwrap_or_else(|| panic!("missing {method_name}"));
        method.body.clone()
    }

    #[test]
    fn analyze_rust_and_typescript_match_the_string_kernel() {
        let mut body = String::from("class Probe\n");
        for (i, entry) in STRING_KERNEL.iter().enumerate() {
            body.push_str(&format!("  def m{i}\n    {}\n  end\n", call_source(entry)));
        }
        body.push_str("end\n");
        for (i, entry) in STRING_KERNEL.iter().enumerate() {
            let body_expr = analyze_method(&body, &format!("m{i}"));
            let send = find_send(&body_expr, entry.name);
            assert_eq!(send.ty, Some(entry.returns.ty()), "{}", entry.name);
            let ExprNode::Send { recv, .. } = &*send.node else {
                panic!("not a send");
            };
            assert_eq!(
                recv.as_ref().and_then(|r| r.ty.clone()),
                Some(Ty::Str),
                "{}",
                entry.name
            );
            let (want_rust, want_ts, want_crystal) = emitted(entry.name);
            assert_eq!(rust_emit(&send), want_rust, "rust {}", entry.name);
            assert_eq!(
                typescript::emit_expr_for_runtime(&send),
                want_ts,
                "typescript {}",
                entry.name
            );
            assert_eq!(
                crystal::emit_expr_for_runtime(&send),
                want_crystal,
                "crystal {}",
                entry.name
            );
        }
    }

    /// A Symbol receiver shares the Rust string bridge. The table is
    /// keyed by String; the shared arm still prints its spelling.
    #[test]
    fn rust_symbol_receiver_uses_the_string_kernel_spelling() {
        let mut recv = Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Sym {
                    value: Symbol::from("ab"),
                },
            },
        );
        recv.ty = Some(Ty::Sym);
        let send = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from("empty?"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        assert_eq!(rust_emit(&send), r#""ab".is_empty()"#);
    }

    /// Wrong arity stays typed, and the Rust bridge declines it.
    #[test]
    fn wrong_arity_stays_typed_and_unbridged() {
        let body = analyze_method(
            "class Probe\n  def wrong\n    \"ab\".upcase(1)\n  end\nend\n",
            "wrong",
        );
        let send = find_send(&body, "upcase");
        assert_eq!(send.ty, Some(Ty::Str));
        let rust = rust_emit(&send);
        assert!(!rust.contains("to_uppercase"), "{rust}");
    }
}
