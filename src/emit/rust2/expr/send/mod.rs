//! Method-call emit — `ExprNode::Send` and its support helpers.
//! Routes call sites by receiver type, applies arg coercion (against
//! callee param types when known), and bridges Ruby stdlib methods to
//! their Rust analogues. The bulk of expr/ emit lives here; the file
//! is large by necessity because `Send` covers method calls, operator
//! desugars, indexing, and most Ruby-stdlib bridging.

use crate::expr::{Expr, ExprNode, Literal};

mod coerce;
mod dispatch;

pub(crate) use coerce::coerce_arg_for_param_ty;
pub(super) use coerce::{cast_via_value_for_union, coerce_arg_for_field_ty};

use coerce::coerce_arg_for_class_method;
use dispatch::{dispatch_method_by_recv_ty, external_class_method_param_tys};

use super::util::{
    coerce_to_value, is_builtin_container_class, is_copy_ty, peel_nil,
    rewrite_method_name, sanitize_ident, synth_default_for_ty,
    value_narrowing_coercion,
};
use super::literal::{emit_hash, emit_is_a};
use super::{
    arg_hash_var_local_ty, class_method_param_ty, current_class_method_param_tys,
    emit_expr, in_class_method, in_constructor, in_module_singleton,
    is_static_method, ivar_field_ty, local_var_ty, module_singleton_slot_name,
    param_ty, recv_var_back_propagated_hash_kv,
};

pub(super) fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    // Constructor `self.field = value` rewrite: `pub fn new(...) ->
    // Self` has no `self` until the closing `Self { ... }` literal,
    // but the lowerer-synthesized `def initialize` body emits
    // `Send { recv: SelfRef, method: "<field>=" }` calls (matching
    // Ruby's `self.col = attrs[:col]` shape). Emit as `let <field>
    // = <value>` so the closing struct literal's shorthand binding
    // picks up the local of the same name. `collect_ivars_assigned_
    // in_body` recognizes this same pattern (instead of just
    // LValue::Ivar / LValue::Attr assigns) so the default-init pass
    // skips fields the body already set.
    if in_constructor()
        && args.len() == 1
        && method.ends_with('=')
        && !method.starts_with('[')
    {
        if let Some(r) = recv {
            if matches!(&*r.node, ExprNode::SelfRef) {
                let field = &method[..method.len() - 1];
                // Coerce the value to the struct field's declared
                // type so the closing `Self { ... }` literal's
                // shorthand binding picks up a same-typed local.
                // Field-position coercion differs from param-position:
                // String fields want owned `String` (not the `&str`
                // that `.as_str().unwrap()` produces for &str params).
                let rhs = match ivar_field_ty(field) {
                    Some(fty) => coerce_arg_for_field_ty(&args[0], &fty),
                    None => emit_expr(&args[0]),
                };
                return format!("let {field} = {rhs}");
            }
        }
    }
    // Stdlib class-method bridges. The Ruby source's `Time.now.utc.iso8601`,
    // `Base64.strict_encode64(...)`, `JSON.generate(...)` patterns
    // refer to Ruby stdlib that doesn't exist in Rust. Recognize the
    // Const-typed receiver and emit the equivalent crate call. The
    // `regex`, `base64`, and `chrono` crates are already rust2 deps.
    if let Some(r) = recv {
        if let ExprNode::Const { path } = &*r.node {
            let last = path.last().map(|s| s.as_str()).unwrap_or("");
            match (last, method, args.len()) {
                ("Time", "now", 0) => {
                    return "chrono::Utc::now()".to_string();
                }
                ("JSON", "generate" | "dump" | "fast_generate", 1) => {
                    return format!("serde_json::to_string(&{}).unwrap()", emit_expr(&args[0]));
                }
                ("JSON", "pretty_generate", 1) => {
                    return format!(
                        "serde_json::to_string_pretty(&{}).unwrap()",
                        emit_expr(&args[0])
                    );
                }
                ("Base64", "encode64" | "strict_encode64", 1) => {
                    return format!(
                        "{{ use base64::Engine; base64::engine::general_purpose::STANDARD.encode({}) }}",
                        emit_expr(&args[0])
                    );
                }
                ("Base64", "urlsafe_encode64", 1) => {
                    return format!(
                        "{{ use base64::Engine; base64::engine::general_purpose::URL_SAFE.encode({}) }}",
                        emit_expr(&args[0])
                    );
                }
                _ => {}
            }
        }
        // `.utc()` on a `Ty::Class { Time }` recv (already a chrono
        // DateTime<Utc> after `Time.now`) is a no-op — chrono's
        // `Utc::now()` is already UTC. `.iso8601()` becomes
        // `.to_rfc3339()`; `.strftime(fmt)` becomes `.format(fmt).to_string()`.
        if matches!(
            r.ty.as_ref().map(peel_nil),
            Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "Time"
        ) {
            match (method, args.len()) {
                ("utc" | "to_time", 0) => return emit_expr(r),
                ("iso8601" | "rfc3339", 0) => return format!("{}.to_rfc3339()", emit_expr(r)),
                ("rfc2822", 0) => return format!("{}.to_rfc2822()", emit_expr(r)),
                ("strftime", 1) => {
                    return format!(
                        "{}.format({}).to_string()",
                        emit_expr(r),
                        emit_expr(&args[0])
                    );
                }
                _ => {}
            }
        }
    }
    // Binary operators (==, !=, <, >, +, -, *, /) ingest as Send
    // with `method` as the operator name. Ruby `a == b` lowers to
    // `Send { recv: a, method: ==, args: [b] }`.
    // Ruby's `+` on strings concatenates; Rust's `&str + &str`
    // doesn't compile (need owned LHS), and `"a" + b + "c"` chains
    // would need cascading allocations. Emit string concatenation
    // as `format!("{}{}", a, b)` — handles every (&str, &str),
    // (&str, String), (String, &str), and chained-format!s as
    // single allocations through format_args!. Recv-type-aware: only
    // fires on Ty::Str/Ty::Sym receivers; numeric `+` keeps its
    // binary-operator emit below.
    if method == "+"
        && recv.is_some()
        && args.len() == 1
        && matches!(
            recv.unwrap().ty.as_ref(),
            Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym)
        )
    {
        return format!(
            "format!(\"{{}}{{}}\", {}, {})",
            emit_expr(recv.unwrap()),
            emit_expr(&args[0]),
        );
    }
    if matches!(method, "==" | "!=" | "<" | ">" | "<=" | ">=" | "+" | "-" | "*" | "/")
        && recv.is_some()
        && args.len() == 1
    {
        return format!("{} {} {}", emit_expr(recv.unwrap()), method, emit_expr(&args[0]));
    }
    // Unary `!` — `!cond` in Ruby lowers as `Send { recv: cond,
    // method: "!", args: [] }`. Rust uses the same `!` operator
    // syntactically but as a prefix unary, not a method call.
    if method == "!" && args.is_empty() {
        if let Some(r) = recv {
            return format!("!{}", emit_expr(r));
        }
    }
    // Array append: `arr << x` Ruby idiom → `arr.push(x)` in Rust.
    // Recv-type-aware: only fires for Vec/Array-typed receivers so
    // user-defined `<<` operators on other types stay intact. The
    // arg is coerced into the elem type so push() type-checks:
    // Vec<String>::push wants owned `String`, but the body-typer
    // often hands us &str literals or borrowed `&str`. `<<` is
    // value-semantic in Ruby (Array#<< takes any object, no borrow
    // distinction), so eager `.to_string()` on the literal side is
    // the right Rust analog. Other elem types pass through unchanged
    // — when more callers surface, extend this match arm in place.
    if method == "<<" && args.len() == 1 {
        if let Some(r) = recv {
            if let Some(crate::ty::Ty::Array { elem }) = r.ty.as_ref() {
                let arg_rendered = match (elem.as_ref(), args[0].ty.as_ref()) {
                    (crate::ty::Ty::Str | crate::ty::Ty::Sym, Some(crate::ty::Ty::Str | crate::ty::Ty::Sym)) => {
                        format!("({}).to_string()", emit_expr(&args[0]))
                    }
                    _ => emit_expr(&args[0]),
                };
                return format!("{}.push({})", emit_expr(r), arg_rendered);
            }
        }
    }
    // Index access: `recv[k]` / `recv[k] = v`. The lowerer shapes
    // both as `Send` with method `[]` / `[]=`; Rust uses the
    // brackets-as-operator form via the `Index` trait. `[]=` lands
    // here for cases not caught by `Assign { target: LValue::Index }`
    // — most commonly `@data[k] = v` (the Ivar-recv case is `Send`
    // because the lowerer hasn't synthesized an LValue::Index for it).
    if let Some(r) = recv {
        if method == "[]" && args.len() == 1 {
            // Peel `Union<T, Nil>` from the recv Ty so receivers bound
            // via `let x = arr[i]` (typed `T | Nil` by the body-typer's
            // Ruby-semantics view) match the same branches as the
            // plain receiver case. Emit chose panic-on-miss for `[]`,
            // so the runtime value really is T.
            let recv_ty = r.ty.as_ref().map(peel_nil);
            let arg_ty = args[0].ty.as_ref().map(peel_nil);
            // Range index on Str/Vec receiver — `pp[1..]`. The Range
            // node emits its endpoints unmodified (`1_i64..`), but
            // slice indexing needs `usize`. Wrap the rendered range
            // in a re-cast so `&pp[(1) as usize..]` compiles.
            if matches!(
                recv_ty,
                Some(crate::ty::Ty::Str)
                    | Some(crate::ty::Ty::Sym)
                    | Some(crate::ty::Ty::Array { .. })
            ) {
                if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                    let b = begin
                        .as_ref()
                        .map(|e| format!("({}) as usize", emit_expr(e)))
                        .unwrap_or_default();
                    let e = end
                        .as_ref()
                        .map(|e| format!("({}) as usize", emit_expr(e)))
                        .unwrap_or_default();
                    let op = if *exclusive { ".." } else { "..=" };
                    let range_s = match (begin.is_some(), end.is_some()) {
                        (true, true) => format!("{b}{op}{e}"),
                        (true, false) => format!("{b}.."),
                        (false, true) => format!("..{e}"),
                        (false, false) => "..".to_string(),
                    };
                    // Str slices need a `&` prefix to yield `&str`; Vec
                    // slices likewise yield `&[T]`. Either way the
                    // caller treats it as borrowed.
                    return format!("&{}[{range_s}]", emit_expr(r));
                }
            }
            // Negative-int literal index on Vec/Str (`arr[-1]` = last
            // element, `arr[-2]` = second to last). Rust's `Index`
            // panics on negative-cast-to-usize (`(-1_i64) as usize`
            // is a huge number). Rewrite to `recv[recv.len() - N]`
            // where N is the absolute negative. Mirrors the TS emit
            // (`recv[recv.length-N]`) at line ~1600 of typescript/expr.rs.
            // Only fires for literal negatives — dynamic negative
            // indices would need a runtime branch, which the framework
            // patterns we ship today don't require.
            if matches!(
                recv_ty,
                Some(crate::ty::Ty::Array { .. })
                    | Some(crate::ty::Ty::Str)
                    | Some(crate::ty::Ty::Sym)
            ) {
                if let ExprNode::Lit { value: Literal::Int { value } } = &*args[0].node {
                    if *value < 0 {
                        let recv_s = emit_expr(r);
                        let abs = -value;
                        // Vec<T>::Index returns `&T`; clone to produce
                        // the owned `T` Ruby's `arr[-1]` semantics
                        // delivers. Callers (e.g. `Base.last`'s tail
                        // wrapped in `Some(...)`) need owned T to
                        // match the `Option<T>` return type.
                        if matches!(recv_ty, Some(crate::ty::Ty::Array { .. })) {
                            return format!(
                                "{recv_s}[{recv_s}.len() - {abs}_usize].clone()"
                            );
                        }
                        return format!("{recv_s}[{recv_s}.len() - {abs}_usize]");
                    }
                }
            }
            // Slice/Vec indexing needs `usize`. Ruby integers (including
            // numeric loop counters like `let mut i = 0_i64`) lower to
            // `i64`; without a cast the `Index<i64>` impl is missing
            // and rustc rejects with E0277. Recv-type-aware: only
            // fires when recv is Ty::Array and the index expression's
            // Ty is Ty::Int. HashMap indexing keeps the bare form
            // (HashMap<K, V> indexes by &K, the user-supplied key
            // is already the right type).
            if let Some(crate::ty::Ty::Array { elem }) = recv_ty {
                if matches!(arg_ty, Some(crate::ty::Ty::Int)) {
                    // `Vec<T>::Index` returns `&T`; passing the result
                    // to a function taking `T` by value (the typical
                    // Ruby-emit consuming-arg shape) requires
                    // materializing an owned T. Append `.clone()` for
                    // non-Copy element types — mirrors the negative-
                    // index branch's clone (the same E0507 motivates
                    // both). Copy elems (i64, f64, bool) need no
                    // suffix.
                    let suffix = if is_copy_ty(elem) { "" } else { ".clone()" };
                    return format!(
                        "{}[({}) as usize]{}",
                        emit_expr(r),
                        emit_expr(&args[0]),
                        suffix
                    );
                }
            }
            // Ty::Class non-builtin recv: route `recv[k]` through the
            // `get_index` method emitted by `sanitize_ident`'s `[]` →
            // `get_index` rewrite. Built-in containers (Hash, Array)
            // and HWIA / Flash / Session keep the bracket form.
            if let Some(crate::ty::Ty::Class { id, .. }) = recv_ty {
                let cls = id.0.as_str();
                if !is_builtin_container_class(cls) {
                    return format!(
                        "{}.get_index({})",
                        emit_expr(r),
                        emit_expr(&args[0])
                    );
                }
            }
            return format!("{}[{}]", emit_expr(r), emit_expr(&args[0]));
        }
        // Ruby `String#[](start, length)` — byte-slice with start +
        // length. Ruby's substring semantics are owned (a fresh
        // `String` each call), so the emit produces owned `String`
        // via `.to_string()` on the slice. Without ownership, a
        // pattern like `ms = padded[0, 3]` reassigns an outer-scope
        // binding to a slice of an inner-scope local (`padded` drops
        // at end of the if-block), tripping E0597. Producing String
        // at the slice site matches Ruby semantics + lets the
        // multi-assign coordination in `analyze::str_color` line up
        // the binding's declared type (`let mut ms: String = …`)
        // with subsequent slice assignments. Args land here as `i64`
        // from the body-typer, hence the `as usize` casts. Caveat:
        // `start_s` is duplicated in the emitted source; fine for
        // the literal/local arg shapes seen in practice (`str[0,
        // 10]`, `str[0, cutoff]`).
        if method == "[]" && args.len() == 2 {
            let recv_s = emit_expr(r);
            let start_s = emit_expr(&args[0]);
            let len_s = emit_expr(&args[1]);
            return format!(
                "(&{recv_s}[({start_s}) as usize..(({start_s}) + ({len_s})) as usize]).to_string()"
            );
        }
        if method == "[]=" && args.len() == 2 {
            // Module-singleton Ivar `[]=`: `@slots[k] = v` in a
            // `def self.foo` body needs to mutate the static
            // `Mutex<Option<HashMap>>` slot through
            // `get_or_insert_with`. The default Ivar-read emit
            // returns a cloned snapshot; bracket-writing into that
            // clone is a silent runtime bug AND fails the surface
            // type-check (`HashMap<String, String>` value type vs
            // `&str` arg, which is what `view_helpers.rs:76/92`
            // reported). Key/value get `.to_string()` appended —
            // the body-typer types `@slots` from the `{}` init as
            // `Hash<Untyped, Untyped>`, so the str_color Hash-K/V
            // coloring doesn't fire here; an unconditional append
            // is idempotent on already-String shapes.
            if in_module_singleton() {
                if let ExprNode::Ivar { name } = &*r.node {
                    let slot = module_singleton_slot_name(name.as_str());
                    let k = emit_expr(&args[0]);
                    let v = emit_expr(&args[1]);
                    return format!(
                        "{{ {slot}.lock().unwrap().get_or_insert_with(std::collections::HashMap::new).insert(({k}).to_string(), ({v}).to_string()); }}"
                    );
                }
            }
            // Mirror the LValue::Index Flash/Session bridge — when
            // the Send-shape `recv.[]=(k, v)` lands on a Flash or
            // Session typed receiver, route through the hand-written
            // `.set(key, value)` method (no IndexMut impl). Per-app
            // model classes implement column-specific `[]=` overrides
            // via the IR's regular method path, not this branch.
            //
            // Type detection covers two channels: `recv.ty` (when the
            // body-typer set it) and an `Ivar { name }` recv whose
            // declared field type sits in IVAR_TYPES.
            let recv_class = match r.ty.as_ref() {
                Some(crate::ty::Ty::Class { id, .. }) => Some(id.0.as_str().to_string()),
                _ => match &*r.node {
                    ExprNode::Ivar { name } => match ivar_field_ty(name.as_str()) {
                        Some(crate::ty::Ty::Class { id, .. }) => {
                            Some(id.0.as_str().to_string())
                        }
                        _ => None,
                    },
                    _ => None,
                },
            };
            if let Some(cls) = recv_class.as_deref() {
                if matches!(cls, "Flash" | "ActionDispatch::Flash") {
                    // Flash::set takes Option<String>; wrap non-Option
                    // rhs in Some(...). See the LValue::Index Flash
                    // branch above — same shape, just the Send-form
                    // entry point (`@flash.[]=(k, v)` in the lowered IR).
                    let rhs = emit_expr(&args[1]);
                    let rhs_is_option = matches!(
                        args[1].ty.as_ref(),
                        Some(crate::ty::Ty::Union { variants })
                            if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
                    );
                    let wrapped = if rhs_is_option {
                        rhs
                    } else {
                        format!("Some({rhs})")
                    };
                    return format!(
                        "{}.set({}, {wrapped})",
                        emit_expr(r),
                        emit_expr(&args[0]),
                    );
                }
                if matches!(cls, "Session" | "ActionDispatch::Session") {
                    return format!(
                        "{}.set({}, {})",
                        emit_expr(r),
                        emit_expr(&args[0]),
                        emit_expr(&args[1]),
                    );
                }
                // Non-builtin Ty::Class — route through `set_index`
                // (the `[]=` operator-method rewrite). Wrap value RHS
                // with Value::from when its Ty isn't already
                // serde_json::Value-shaped.
                if !is_builtin_container_class(cls) {
                    let rhs = emit_expr(&args[1]);
                    let coerced_rhs = coerce_to_value(&args[1], &rhs);
                    return format!(
                        "{}.set_index({}, {coerced_rhs})",
                        emit_expr(r),
                        emit_expr(&args[0]),
                    );
                }
            }
            // Ty::Hash recv via Send-`[]=` — HashMap doesn't implement
            // IndexMut, so `recv[k] = v` syntax fails (E0594). Mirror
            // the LValue::Index Ty::Hash branch and route through
            // `.insert(k, v)`. Wrap in `{ ...; }` for the no-else
            // `if cond { ... }` context (HashMap::insert returns
            // Option<V>; trailing `;` makes the block `()` typed).
            //
            // K/V coercion via `local_var_ty`: when the recv is a Var
            // whose locally-tracked type (e.g. back-propagated from
            // the function's return signature, see
            // `empty_hash_return_ty`) is `Hash<Str|Sym, Str|Sym>`, the
            // body-typer's IR-level `r.ty` may still be the
            // `Hash<Untyped, Untyped>` shape from the `{}` init — so
            // str_color's Hash-recv coloring doesn't fire and the args
            // arrive un-coerced. Append `.to_string()` per arg whose
            // K/V slot is String-shaped. Idempotent for already-String.
            let recv_hash_kv = recv_var_back_propagated_hash_kv(r);
            let is_hash_recv =
                matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
                    || recv_hash_kv.is_some();
            if is_hash_recv {
                let coerce_str = |slot: Option<&crate::ty::Ty>, raw: String| -> String {
                    match slot {
                        Some(crate::ty::Ty::Str | crate::ty::Ty::Sym) => {
                            format!("({raw}).to_string()")
                        }
                        _ => raw,
                    }
                };
                let k = emit_expr(&args[0]);
                let v = emit_expr(&args[1]);
                let (kk, vv) = match &recv_hash_kv {
                    Some((k_ty, v_ty)) => (
                        coerce_str(Some(k_ty), k),
                        coerce_str(Some(v_ty), v),
                    ),
                    None => (k, v),
                };
                return format!("{{ {}.insert({kk}, {vv}); }}", emit_expr(r));
            }
            return format!("{}[{}] = {}", emit_expr(r), emit_expr(&args[0]), emit_expr(&args[1]));
        }
        // Ruby `value.is_a?(Class)` runtime type check. Rust has no
        // generic analog — every type is statically known. For the
        // `serde_json::Value`-typed gradual-escape recv (the common
        // shape after Ty::Untyped commits to `serde_json::Value`),
        // map the known Ruby class names to serde_json predicates;
        // user-defined classes degrade to `false` with a comment
        // (always-false branch in a chain like normalize_value, the
        // next branch handles the real case).
        if method == "is_a?" && args.len() == 1 {
            return emit_is_a(r, &args[0]);
        }
        // `hash.to_h` — Ruby identity on Hash (returns self). Crystal
        // uses it to bridge NamedTuple → Hash; Rust has no NamedTuple,
        // so on a HashMap-typed recv the method has no analog and
        // `.clone()` preserves the "fresh owned hash" semantics. The
        // Flash/Session typed structs have their own `to_h()` method
        // returning HashMap<String, String>; those go through the
        // generic dispatch path below (recv ty is not Ty::Hash there).
        // Recv typed Untyped/None falls through too — call sites that
        // need the .to_h() on a serde_json::Value will rely on
        // separate emit work for the Value method-call fix.
        if method == "to_h"
            && args.is_empty()
            && matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
        {
            return format!("{}.clone()", emit_expr(r));
        }
        // `hash.merge(other)` — Ruby Hash#merge returns a new Hash
        // with `other`'s entries layered on top. Rust HashMap has no
        // built-in merge AND the typical call site has mixed K/V
        // types (literal `(&str, &str)` merged with parameter
        // `HashMap<String, Value>`), which a generic trait can't
        // bridge. `merge_attrs` from runtime/rust/hash_ext.rs takes
        // both sides as IntoIterator over (K: Into<String>, V:
        // Into<Value>) and produces a unified
        // `HashMap<String, Value>` — matches what `render_attrs`,
        // `r#where`, and similar consumers expect. Recv-types
        // outside Ty::Hash (Flash, Session) keep their own `merge`
        // method via the generic dispatch path below.
        if method == "merge"
            && args.len() == 1
            && matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
        {
            return format!(
                "merge_attrs({}, {})",
                emit_expr(r),
                emit_expr(&args[0]),
            );
        }
        // `hash.fetch(key, default)` — Ruby Hash#fetch returns the
        // value at key or `default` when missing. Rust HashMap has
        // `.get(K) -> Option<&V>`; bridge via `.cloned()` (nil
        // default; `Option::None` unifies trivially) or
        // `.cloned().unwrap_or(default)` otherwise. Recv must be a
        // Ty::Hash that's a real value at the call site.
        // Const receivers (e.g. `STATUS_CODES.fetch(...)`) are
        // accepted now that module-level Hash constants emit as
        // `static NAME: LazyLock<HashMap<...>>` — the LazyLock
        // auto-deref makes `STATUS_CODES.get(&k)` resolve through to
        // the inner HashMap.
        if method == "fetch"
            && args.len() == 2
            && matches!(r.ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Hash { .. }))
        {
            let recv_s = emit_expr(r);
            let key_s = emit_expr(&args[0]);
            let default_is_nil = matches!(
                &*args[1].node,
                ExprNode::Lit { value: Literal::Nil }
            );
            if default_is_nil {
                return format!("{recv_s}.get({key_s}).cloned()");
            }
            let default_s = emit_expr(&args[1]);
            return format!("{recv_s}.get({key_s}).cloned().unwrap_or({default_s})");
        }
        // `s.tr(from, to)` — character translation. Limited to
        // single-char from/to (the framework Ruby use case is
        // `inner_k.to_s.tr("_", "-")` in render_attrs). Multi-char
        // and ranges fall through to generic dispatch so the gap
        // surfaces as a compile error rather than silently mis-
        // emitting. Mirrors the TypeScript emit's `tr` peephole.
        if method == "tr" && args.len() == 2 {
            if let (
                ExprNode::Lit { value: Literal::Str { value: from } },
                ExprNode::Lit { value: Literal::Str { value: to } },
            ) = (&*args[0].node, &*args[1].node)
            {
                if from.chars().count() == 1 && to.chars().count() == 1 {
                    let recv_s = emit_expr(r);
                    return format!("{recv_s}.replace({from:?}, {to:?})");
                }
            }
        }
        // `str.split(sep)` — Ruby returns an Array; Rust returns a
        // lazy `Split` iterator that doesn't support `.len()` or
        // indexing. Eagerly collect to Vec<&str> so downstream
        // `parts.length` / `parts[i]` (the typical router-table
        // walking pattern) compiles. Recv-type-aware: only fires on
        // Ty::Str receivers; user-defined `split` on other types
        // stays intact.
        if method == "split"
            && args.len() == 1
            && matches!(r.ty.as_ref(), Some(crate::ty::Ty::Str))
        {
            let recv_s = emit_expr(r);
            let sep_s = emit_expr(&args[0]);
            return format!("{recv_s}.split({sep_s}).collect::<Vec<&str>>()");
        }
        // `s.gsub(pattern, table)` with a `Ty::Class { Regexp }` first
        // arg and a `Ty::Hash` second arg — the canonical Ruby idiom
        // for table-driven character replacement (used by
        // `view_helpers.html_escape` and `json_builder.encode_string`).
        // Rust analog: `pattern.replace_all(&s, |caps| table[&caps[0]])`
        // returning the owned String. The pattern is typically a
        // module-level `LazyLock<Regex>` constant, the table a
        // `LazyLock<HashMap<&'static str, &'static str>>` — both auto-
        // deref to their inner types at the call site.
        if method == "gsub"
            && args.len() == 2
            && matches!(r.ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
            && matches!(
                args[0].ty.as_ref().map(peel_nil),
                Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "Regexp"
            )
            && matches!(args[1].ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Hash { .. }))
        {
            let recv_s = emit_expr(r);
            let pat_s = emit_expr(&args[0]);
            let table_s = emit_expr(&args[1]);
            return format!(
                "{pat_s}.replace_all(&{recv_s}, |__caps: &regex::Captures| -> String \
                 {{ (*{table_s}.get(&__caps[0]).unwrap_or(&\"\")).to_string() }}).into_owned()"
            );
        }
        // `s.gsub(needle, replacement)` — both String args. Ruby
        // returns a new string with all occurrences substituted;
        // Rust's `str::replace(needle, replacement)` is the direct
        // analog (same all-occurrences semantics, returns owned String).
        if method == "gsub"
            && args.len() == 2
            && matches!(r.ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
            && matches!(args[0].ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
            && matches!(args[1].ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
        {
            let recv_s = emit_expr(r);
            let needle_s = emit_expr(&args[0]);
            let repl_s = emit_expr(&args[1]);
            return format!("{recv_s}.replace({needle_s}, {repl_s})");
        }
        // `arr.length` / `str.length` — Ruby returns Integer.
        // Rust's `.len()` returns `usize`, but Ruby Integers lower
        // to `i64` everywhere else (`while i < arr.length`, `if
        // arr.length == 0`). Emit as `(recv.len() as i64)` on
        // sized receivers so downstream i64 arithmetic / comparison
        // compiles without a per-call-site widen. Untyped receivers
        // fall through to the generic `.length -> .len()` bridge
        // (their value-shape may not even support `.len()`).
        if method == "length"
            && args.is_empty()
            && matches!(
                r.ty.as_ref(),
                Some(crate::ty::Ty::Array { .. })
                    | Some(crate::ty::Ty::Hash { .. })
                    | Some(crate::ty::Ty::Str)
                    | Some(crate::ty::Ty::Sym)
            )
        {
            return format!("({}.len() as i64)", emit_expr(r));
        }
        // Recv-Ty-aware method bridge — mirrors the structure of
        // `typescript/expr.rs`'s match-on-recv-ty around lines
        // 1972–2245. Each Ruby method on Array/Str/Sym/Hash gets a
        // Rust-shaped lowering; unrecognized names fall through to
        // the generic dispatch + rewrite_method_name table below.
        // Recv-aware so user-defined methods of the same name on
        // other types still resolve through the regular path.
        if let Some(rendered) = dispatch_method_by_recv_ty(r, method, args) {
            return rendered;
        }
        // `str.capitalize` — Ruby's "first letter uppercase, rest
        // lowercase". Rust's String has no direct analog; inline a
        // small block that chains uppercase-first + lowercase-rest.
        // Fires whenever `.capitalize()` is called with no args; the
        // body-typer doesn't always propagate Ty::Str through getter
        // Sends (e.g. `self.model_name.capitalize` where model_name
        // is an attr_reader returning String), so checking recv.ty
        // misses those cases. Non-String receivers would already
        // fail E0599 today and surface as a clearer error after the
        // bridge fires (`&recv_s` deref against a non-String type).
        if method == "capitalize" && args.is_empty() {
            let recv_s = emit_expr(r);
            return format!(
                "{{ let __s: &str = &{recv_s}; let mut __c = __s.chars(); \
                    match __c.next() {{ \
                        Some(__f) => __f.to_uppercase().collect::<String>() + &__c.as_str().to_lowercase(), \
                        None => String::new(), \
                    }} }}"
            );
        }
        // `value.nil?` on a `Ty::Untyped` or unresolved-Var receiver —
        // `serde_json::Value` exposes `.is_null()` (not `.is_none`,
        // which is the Option method the generic `nil?` bridge below
        // produces). The Var-typed case covers receivers the body-
        // typer didn't fully resolve (e.g. `value = @model[field]`
        // where `@model[field]` is typed Untyped per Base's RBS but
        // the local-let propagation leaves `value`'s recv ty
        // unresolved at the emit-walk's view of the Var-read site).
        // The generic bridge stays in place for true Option-typed
        // receivers (typical Ruby `attr_reader` getters typed `T?`).
        if method == "nil?"
            && args.is_empty()
            && matches!(
                r.ty.as_ref(),
                Some(crate::ty::Ty::Untyped) | Some(crate::ty::Ty::Var { .. })
            )
        {
            return format!("{}.is_null()", emit_expr(r));
        }
        // `value.nil?` on a non-nilable primitive — body-typer says
        // `Ty::Str` / `Ty::Int` / etc. Ruby/Crystal need the runtime
        // check (Crystal's `property title : String?` is nilable; Ruby
        // attrs default to nil), but rust2's struct field is the bare
        // owned type with no `Option<...>` wrapper, so `.is_none()`
        // would E0599. The static answer here is `false` — emit a
        // constant and let LLVM fold the surrounding If. Side-effect-
        // free recvs (`self.title`) drop out; expression-shaped recvs
        // (a Send chain) need a tiny bind-and-discard, but the body
        // -typer's nil? targets are almost always Ivar reads. Inline
        // the bind only when the recv isn't a pure read.
        if method == "nil?"
            && args.is_empty()
            && matches!(
                r.ty.as_ref(),
                Some(
                    crate::ty::Ty::Str
                        | crate::ty::Ty::Sym
                        | crate::ty::Ty::Int
                        | crate::ty::Ty::Float
                        | crate::ty::Ty::Bool
                )
            )
        {
            let recv_pure = matches!(
                &*r.node,
                ExprNode::Ivar { .. }
                    | ExprNode::Var { .. }
                    | ExprNode::SelfRef
                    | ExprNode::Lit { .. }
                    | ExprNode::Const { .. }
            );
            if recv_pure {
                return "false".to_string();
            }
            return format!("{{ let _ = {}; false }}", emit_expr(r));
        }
        // `self.class.X(args)` — Ruby idiom for "dispatch X on the
        // class of self" (`@id` getter is an instance dispatch;
        // `table_name`, `schema_columns` are per-subclass class
        // methods). The chained Send `recv.class.X` lowers to
        // `Send { recv: Send { recv: SelfRef, method: "class" },
        // method: X }`. In Rust, the equivalent is `Self::X(args)`
        // — the surrounding `impl Base` resolves the per-class
        // override at the call site. Only fires when the recv is
        // SelfRef (subclass overrides reach the correct method
        // through Self resolution); other receivers' .class chains
        // surface as proper E0599 noise upstream.
        if let ExprNode::Send {
            recv: Some(inner_recv),
            method: inner_method,
            args: inner_args,
            ..
        } = &*r.node
        {
            if inner_method.as_str() == "class"
                && inner_args.is_empty()
                && matches!(&*inner_recv.node, ExprNode::SelfRef)
            {
                let rewritten = rewrite_method_name(method);
                let args_s: Vec<String> = args.iter().map(emit_expr).collect();
                if args_s.is_empty() {
                    return format!("Self::{rewritten}()");
                }
                return format!("Self::{rewritten}({})", args_s.join(", "));
            }
        }
    }
    // Ruby/Rust method-name bridge. Sanitize predicates (`foo?` →
    // `foo`, `foo!` → `foo`) since Rust identifiers reject those
    // suffixes. The user-defined HWIA methods `key?`/`has_key?`/etc.
    // pair with the matching `pub fn` rename in `method.rs` so def
    // and call sites stay aligned. A small set of Ruby stdlib calls
    // (`to_s`, `length`, `nil?`, `key?` on Hash, etc.) needs a
    // different Rust name; rewrite those here. Caveat: receiver-type-
    // sensitive bridges (Hash#key? vs user-defined `key?`) collapse
    // to the generic form — Rust's `contains_key` for HashMap vs
    // the user's stripped `key` may emit ambiguously when the recv
    // is untyped serde_json::Value. Live with the noise until type-
    // aware bridging lands.
    let rewritten_method = rewrite_method_name(method);
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    // Free functions / module functions (Inflector.pluralize → bare
    // pluralize() in the inflector module). Implicit-self bare calls
    // emit as bare function calls.
    if recv.is_none() {
        // `require "X"` inside a method body — Ruby's lazy load
        // statement. Rust resolves cross-file deps through top-level
        // `use` imports (the runtime_loader's `imports` field), so
        // the inline `require` has nothing to do at runtime. Emit as
        // a comment so the line stays inert.
        if method == "require" {
            let arg_repr = args_s.join(", ");
            return format!("/* require({arg_repr}) — no-op in rust2 */");
        }
        // Ruby's class-method `new` (implicit-self call to Class#new
        // inside a `def self.X` body). Lowers to `Send { recv: None,
        // method: "new" }`. Rust analog inside an `impl Type` is
        // `Self::new(args)` — the constructor's canonical Rust name
        // (matches `emit_instance_method`'s `is_init` lowering).
        if method == "new" && in_class_method() {
            return format!("Self::new({})", args_s.join(", "));
        }
        return format!("{}({})", rewritten_method, args_s.join(", "));
    }
    let r = recv.unwrap();
    // Static-method routing: `self.method(args)` where `method` was
    // classified as not-reading-self emits as `Self::method(args)`.
    // Required inside `pub fn new` (no instance yet), and also a
    // valid choice elsewhere for inherently-static helpers — Rust
    // accepts both `obj.foo()` and `T::foo(...)` when `foo` doesn't
    // take a receiver, but the static form is unambiguous.
    //
    // The same routing applies unconditionally inside class methods
    // (`def self.X` bodies): Ruby's `self` *is* the class there, so
    // every `self.method(args)` is class-level dispatch.
    if matches!(&*r.node, ExprNode::SelfRef) && (is_static_method(method) || in_class_method()) {
        // Callee-back-propagation: when the callee's declared param[i]
        // is `Hash<K, V>` and the arg expression is a Var whose
        // `local_var_ty` is a different `Hash<K', V'>` (or
        // body-typer-derived but with mismatched K/V), insert a
        // `into_iter().map().collect()` transform. The button_to →
        // render_attrs(form_attrs) pattern is the canonical case:
        // form_attrs is locally `HashMap<&str, String>` (from
        // `{action: …, method: "post"}.to_h`), render_attrs takes
        // `HashMap<String, serde_json::Value>`.
        let coerced: Vec<String> = args
            .iter()
            .enumerate()
            .map(|(i, a)| coerce_arg_for_class_method(method, i, a))
            .collect();
        if coerced.is_empty() {
            return format!("Self::{rewritten_method}()");
        }
        return format!("Self::{rewritten_method}({})", coerced.join(", "));
    }
    // Callee-back-propagation for two recv shapes:
    //
    // 1. **SelfRef instance method** (`self.set_id(arg)`): callee is
    //    a sibling method on the current class. Use
    //    `CLASS_METHOD_PARAM_TYS` (populated by `library.rs` at class
    //    emit start) to look up the param Tys. Closes the lowered
    //    model `self.set_id(row["id"])` shape (Value → i64 coercion).
    // 2. **Const class method** (`Db::escape_string(self.body)`):
    //    callee is in a hand-written runtime module not surfaced
    //    through the per-class registry. Hardcoded
    //    `external_class_method_param_tys` covers Db today; future
    //    modules add entries as their sites surface.
    let final_args: Vec<String> = if matches!(&*r.node, ExprNode::SelfRef) {
        args.iter()
            .enumerate()
            .map(|(i, a)| coerce_arg_for_class_method(method, i, a))
            .collect()
    } else if let ExprNode::Const { path } = &*r.node {
        let class = path.last().map(|s| s.as_str()).unwrap_or("");
        // Try the hand-written runtime sigs first (Db, Broadcasts),
        // then fall back to the current class's method table. The
        // latter covers `Self::new(...)` / `Class::method(...)` shapes
        // where path.last() matches the class currently being
        // emitted — `Article::new()` from inside `impl Article`.
        let param_tys = external_class_method_param_tys(class, method)
            .or_else(|| current_class_method_param_tys(method));
        if let Some(param_tys) = param_tys {
            let mut out: Vec<String> = Vec::with_capacity(param_tys.len().max(args.len()));
            for (i, _) in param_tys.iter().enumerate() {
                match (args.get(i), param_tys.get(i)) {
                    // Caller-supplied arg: apply per-param coercion.
                    (Some(a), Some(pt)) => out.push(coerce_arg_for_param_ty(a, pt)),
                    // Missing trailing arg: synthesize a default for
                    // shapes that have one (Hash → HashMap::new(),
                    // etc.). Ruby `def initialize(attrs = {})`
                    // accepts zero-arg `Article.new`; Rust needs the
                    // explicit default. Without this `Class::new()`
                    // sites trip E0061.
                    (None, Some(pt)) => match synth_default_for_ty(pt) {
                        Some(d) => out.push(d),
                        None => break,
                    },
                    _ => break,
                }
            }
            // If caller passed MORE args than callee declares (rare —
            // splat / overload patterns), append the extras un-coerced.
            for a in args.iter().skip(param_tys.len()) {
                out.push(emit_expr(a));
            }
            out
        } else {
            args_s
        }
    } else if matches!(r.ty.as_ref(), Some(crate::ty::Ty::Class { .. }))
        && method.ends_with('=')
        && args.len() == 1
    {
        // Setter convention coercion for instance-Send recvs whose Ty
        // is a known class: `instance.set_<col>(value)` came from the
        // model lowerer's `attr_writer` shim, where the setter param
        // ty equals the column ty. rust2 emits `Ty::Str/Sym` params
        // as `&str`, but the lowerer hands String-shaped args at
        // sites like `instance.set_body(row.body())` (where
        // `row.body()` returns owned `String`). Wrap owned-String
        // sources with `&(...)` so the borrow matches. Non-Str
        // setter args (`set_id(i64)`) pass through unchanged.
        //
        // Heuristic — the Const-recv arm above uses an explicit
        // param-Tys table; for instance-Sends we don't have a global
        // sibling registry yet, so the setter-name + arg-Ty + owned-
        // node combo carries the same signal. Limited to one-arg
        // calls because that's the AR `set_<col>` shape; broader
        // setter shapes (multi-arg) can opt in later.
        let mut out: Vec<String> = Vec::with_capacity(1);
        let coerced = coerce_arg_for_param_ty(
            &args[0],
            // Use the arg's body-typer Ty as the param Ty: setter
            // params for Str cols are typed Str, matching the row
            // accessor's return Ty. For non-Str args the coerce
            // function returns the bare emit.
            args[0].ty.as_ref().unwrap_or(&crate::ty::Ty::Untyped),
        );
        out.push(coerced);
        out
    } else {
        args_s
    };
    let recv_s = emit_expr(r);
    // Static method dispatch — `Type.method(args)` in Ruby becomes
    // `Type::method(args)` in Rust when the receiver is a Const
    // (class/module reference). The `.` form binds to a value
    // receiver; `::` binds to a type.
    let dispatch = if matches!(&*r.node, ExprNode::Const { .. }) {
        "::"
    } else {
        "."
    };
    if final_args.is_empty() {
        format!("{recv_s}{dispatch}{rewritten_method}()")
    } else {
        format!("{recv_s}{dispatch}{rewritten_method}({})", final_args.join(", "))
    }
}

